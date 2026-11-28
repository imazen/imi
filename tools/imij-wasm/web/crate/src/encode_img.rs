//! Zero-dependency browser re-encoders from decoded pixels to uncompressed BMP
//! and uncompressed store-mode PNG (DEFLATE `BTYPE=00` stored blocks) with CICP
//! and optional Display-P3 ICC metadata.
//!
//! Designed for browser `<img src="blob:...">` polyfill injection where the goal
//! is sub-millisecond framing on a Web Worker without any DEFLATE compression CPU.

#![forbid(unsafe_code)]

use imij::{Cicp, Error};

/// Build a canonical 492-byte ICC v4.3 Display-P3 profile (exact Bradford
/// D65->D50 primaries summing to PCS D50, exact sRGB `para` funcType=3 TRC).
pub(crate) fn display_p3_icc_v4() -> Vec<u8> {
    const SIZE: usize = 492;
    let mut out = vec![0u8; SIZE];
    let put_u16 = |buf: &mut [u8], at: usize, v: u16| {
        buf[at..at + 2].copy_from_slice(&v.to_be_bytes());
    };
    let put_u32 = |buf: &mut [u8], at: usize, v: u32| {
        buf[at..at + 4].copy_from_slice(&v.to_be_bytes());
    };
    let put_i32 = |buf: &mut [u8], at: usize, v: i32| {
        buf[at..at + 4].copy_from_slice(&v.to_be_bytes());
    };

    // 128-byte ICC v4.3 header
    put_u32(&mut out, 0, SIZE as u32);
    put_u32(&mut out, 8, 0x0430_0000); // v4.3.0.0
    out[12..16].copy_from_slice(b"mntr");
    out[16..20].copy_from_slice(b"RGB ");
    out[20..24].copy_from_slice(b"XYZ ");
    put_u16(&mut out, 24, 2026);
    put_u16(&mut out, 26, 1);
    put_u16(&mut out, 28, 1);
    out[36..40].copy_from_slice(b"acsp");
    // PCS illuminant D50 (0.9642, 1.0, 0.8249)
    put_i32(&mut out, 68, 0x0000_F6D6);
    put_i32(&mut out, 72, 0x0001_0000);
    put_i32(&mut out, 76, 0x0000_D32D);

    // Tag table: 10 tags (bytes 128..252)
    let tags: [([u8; 4], u32, u32); 10] = [
        (*b"desc", 252, 48),
        (*b"cprt", 300, 34),
        (*b"wtpt", 336, 20),
        (*b"chad", 356, 44),
        (*b"rXYZ", 400, 20),
        (*b"gXYZ", 420, 20),
        (*b"bXYZ", 440, 20),
        (*b"rTRC", 460, 32),
        (*b"gTRC", 460, 32),
        (*b"bTRC", 460, 32),
    ];
    put_u32(&mut out, 128, tags.len() as u32);
    for (idx, &(sig, off, len)) in tags.iter().enumerate() {
        let base = 132 + idx * 12;
        out[base..base + 4].copy_from_slice(&sig);
        put_u32(&mut out, base + 4, off);
        put_u32(&mut out, base + 8, len);
    }

    // desc: mluc "Display P3" (252..300)
    let write_mluc = |buf: &mut [u8], at: usize, text: &str| {
        buf[at..at + 4].copy_from_slice(b"mluc");
        put_u32(buf, at + 8, 1); // record count
        put_u32(buf, at + 12, 12); // record size
        buf[at + 16..at + 20].copy_from_slice(b"enUS");
        put_u32(buf, at + 20, (text.len() * 2) as u32);
        put_u32(buf, at + 24, 28);
        for (i, ch) in text.bytes().enumerate() {
            put_u16(buf, at + 28 + i * 2, u16::from(ch));
        }
    };
    write_mluc(&mut out, 252, "Display P3");
    write_mluc(&mut out, 300, "CC0");

    let write_xyz = |buf: &mut [u8], at: usize, x: i32, y: i32, z: i32| {
        buf[at..at + 4].copy_from_slice(b"XYZ ");
        put_i32(buf, at + 8, x);
        put_i32(buf, at + 12, y);
        put_i32(buf, at + 16, z);
    };
    // wtpt: D50
    write_xyz(&mut out, 336, 0x0000_F6D6, 0x0001_0000, 0x0000_D32D);

    // chad: Bradford chromatic adaptation D65 -> D50 (356..400)
    out[356..360].copy_from_slice(b"sf32");
    let chad: [i32; 9] = [68674, 1502, -3291, 1939, 64912, -1119, -606, 988, 49262];
    for (i, &v) in chad.iter().enumerate() {
        put_i32(&mut out, 364 + i * 4, v);
    }

    // D50-adapted Display-P3 primaries (rXYZ + gXYZ + bXYZ == D50 exactly)
    write_xyz(&mut out, 400, 33759, 15807, -69);
    write_xyz(&mut out, 420, 19135, 45366, 2745);
    write_xyz(&mut out, 440, 10296, 4363, 51385);

    // para funcType=3 sRGB transfer curve shared by rTRC/gTRC/bTRC (460..492)
    out[460..464].copy_from_slice(b"para");
    put_u16(&mut out, 468, 3); // funcType 3: Y = (aX + b)^g for X >= d, cX for X < d
    put_i32(&mut out, 472, 157_286); // g = 2.4
    put_i32(&mut out, 476, 62_119); // a = 1 / 1.055
    put_i32(&mut out, 480, 3_417); // b = 0.055 / 1.055 (a + b == 65536)
    put_i32(&mut out, 484, 5_072); // c = 1 / 12.92
    put_i32(&mut out, 488, 2_651); // d = 0.04045

    out
}

pub(crate) fn is_display_p3(cicp: &Cicp) -> bool {
    cicp.color_primaries == 12
        && cicp.transfer_characteristics == 13
        && cicp.matrix_coefficients == 0
        && cicp.full_range
}

/// Encode tightly packed RGB8 (`channels == 3`) or RGBA8 (`channels == 4`)
/// into an uncompressed bottom-up BMP.
///
/// - When `channels == 3` and `!embed_icc`: 40-byte `BITMAPINFOHEADER`, 24-bit BGR.
/// - When `channels == 4` or `embed_icc`: 124-byte `BITMAPV5HEADER` (`BI_BITFIELDS`
///   for 32-bit BGRA, `BI_RGB` for 24-bit BGR) with `LCS_PROFILE_EMBEDDED` +
///   Display-P3 ICC profile when `display_p3 && embed_icc`, else `LCS_sRGB`.
pub(crate) fn encode_bmp(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: usize,
    display_p3: bool,
    embed_icc: bool,
) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 || !matches!(channels, 3 | 4) {
        return Err(Error::InvalidInput("invalid BMP geometry or channels"));
    }
    let w = width as usize;
    let h = height as usize;
    let src_row = w.checked_mul(channels).ok_or(Error::TooLarge)?;
    let expected = src_row.checked_mul(h).ok_or(Error::TooLarge)?;
    if pixels.len() != expected {
        return Err(Error::InvalidInput("BMP pixel buffer length mismatch"));
    }

    let use_v5 = channels == 4 || (display_p3 && embed_icc);
    let dib_size: usize = if use_v5 { 124 } else { 40 };
    let icc = if use_v5 && display_p3 && embed_icc {
        display_p3_icc_v4()
    } else {
        Vec::new()
    };
    let pixel_offset = 14usize + dib_size;
    let bpp = channels;
    let dst_row = (w.checked_mul(bpp).ok_or(Error::TooLarge)?)
        .checked_add(3)
        .ok_or(Error::TooLarge)?
        & !3;
    let pixel_bytes = dst_row.checked_mul(h).ok_or(Error::TooLarge)?;
    let total = pixel_offset
        .checked_add(pixel_bytes)
        .and_then(|n| n.checked_add(icc.len()))
        .ok_or(Error::TooLarge)?;
    let total_u32 = u32::try_from(total).map_err(|_| Error::TooLarge)?;
    let pixel_bytes_u32 = u32::try_from(pixel_bytes).map_err(|_| Error::TooLarge)?;

    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| Error::TooLarge)?;
    out.resize(total, 0);

    let put_u16 = |buf: &mut [u8], at: usize, v: u16| {
        buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
    };
    let put_u32 = |buf: &mut [u8], at: usize, v: u32| {
        buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };
    let put_i32 = |buf: &mut [u8], at: usize, v: i32| {
        buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    };

    // 14-byte BITMAPFILEHEADER
    out[0] = b'B';
    out[1] = b'M';
    put_u32(&mut out, 2, total_u32);
    put_u32(&mut out, 10, pixel_offset as u32);

    // DIB header (BITMAPINFOHEADER / BITMAPV5HEADER)
    put_u32(&mut out, 14, dib_size as u32);
    put_i32(&mut out, 18, width as i32);
    put_i32(&mut out, 22, height as i32); // bottom-up (maximally compatible with V5 ICC)
    put_u16(&mut out, 26, 1); // planes
    put_u16(&mut out, 28, (bpp * 8) as u16); // bit count
    // BI_BITFIELDS (3) for 32-bit BGRA so alpha mask is explicit; BI_RGB (0) for 24-bit BGR
    let compression: u32 = if channels == 4 { 3 } else { 0 };
    put_u32(&mut out, 30, compression);
    put_u32(&mut out, 34, pixel_bytes_u32);
    put_i32(&mut out, 38, 2835); // 72 DPI
    put_i32(&mut out, 42, 2835);

    if use_v5 {
        if channels == 4 {
            // Channel masks for little-endian BGRA bytes [B, G, R, A]
            put_u32(&mut out, 54, 0x00FF_0000); // bV5RedMask
            put_u32(&mut out, 58, 0x0000_FF00); // bV5GreenMask
            put_u32(&mut out, 62, 0x0000_00FF); // bV5BlueMask
            put_u32(&mut out, 66, 0xFF00_0000); // bV5AlphaMask
        }
        if !icc.is_empty() {
            // LCS_PROFILE_EMBEDDED ('MBED' = 0x4D424544)
            put_u32(&mut out, 70, 0x4D42_4544);
            // bV5Intent = LCS_GM_IMAGES (4, perceptual)
            put_u32(&mut out, 122, 4);
            // bV5ProfileData is offset from start of BITMAPV5HEADER (byte 14)
            let profile_offset = (dib_size + pixel_bytes) as u32;
            put_u32(&mut out, 126, profile_offset);
            put_u32(&mut out, 130, icc.len() as u32);
        } else {
            // LCS_sRGB ('sRGB' = 0x73524742)
            put_u32(&mut out, 70, 0x7352_4742);
            put_u32(&mut out, 122, 4);
        }
    }

    // Write pixel rows bottom-up (BGR / BGRA)
    if channels == 3 {
        for y in 0..h {
            let src = &pixels[(h - 1 - y) * src_row..(h - y) * src_row];
            let dst = &mut out[pixel_offset + y * dst_row..pixel_offset + y * dst_row + w * 3];
            for (s, d) in src
                .as_chunks::<3>()
                .0
                .iter()
                .zip(dst.as_chunks_mut::<3>().0.iter_mut())
            {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
            }
        }
    } else {
        for y in 0..h {
            let src = &pixels[(h - 1 - y) * src_row..(h - y) * src_row];
            let dst = &mut out[pixel_offset + y * dst_row..pixel_offset + y * dst_row + w * 4];
            for (s, d) in src
                .as_chunks::<4>()
                .0
                .iter()
                .zip(dst.as_chunks_mut::<4>().0.iter_mut())
            {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
        }
    }

    if !icc.is_empty() {
        let icc_at = pixel_offset + pixel_bytes;
        out[icc_at..icc_at + icc.len()].copy_from_slice(&icc);
    }

    Ok(out)
}

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            if c & 1 != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

pub(crate) fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    crc = !crc;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

pub(crate) fn adler32_update(state: (u32, u32), data: &[u8]) -> (u32, u32) {
    const MOD: u32 = 65_521;
    const NMAX: usize = 5552;
    let (mut s1, mut s2) = state;
    for chunk in data.chunks(NMAX) {
        for &b in chunk {
            s1 += u32::from(b);
            s2 += s1;
        }
        s1 %= MOD;
        s2 %= MOD;
    }
    (s1, s2)
}

fn push_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let crc = crc32_update(crc32_update(0, tag), data);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode tightly packed RGB8 (`channels == 3`) or RGBA8 (`channels == 4`)
/// into an uncompressed store-mode PNG (DEFLATE `BTYPE=00` stored blocks) with
/// `cICP` chunk and optional Display-P3 `iCCP` chunk.
pub(crate) fn encode_store_png(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: usize,
    cicp: Cicp,
    include_iccp: bool,
) -> Result<Vec<u8>, Error> {
    if width == 0 || height == 0 || !matches!(channels, 3 | 4) {
        return Err(Error::InvalidInput("invalid PNG geometry or channels"));
    }
    let w = width as usize;
    let h = height as usize;
    let row_bytes = w.checked_mul(channels).ok_or(Error::TooLarge)?;
    let expected = row_bytes.checked_mul(h).ok_or(Error::TooLarge)?;
    if pixels.len() != expected {
        return Err(Error::InvalidInput("PNG pixel buffer length mismatch"));
    }

    let raw_row = row_bytes.checked_add(1).ok_or(Error::TooLarge)?; // filter byte 0 + row
    let raw_total = raw_row.checked_mul(h).ok_or(Error::TooLarge)?;
    let block_count = raw_total.div_ceil(65_535).max(1);
    let zlib_len = 2usize
        .checked_add(raw_total)
        .and_then(|n| n.checked_add(block_count.checked_mul(5)?))
        .and_then(|n| n.checked_add(4))
        .ok_or(Error::TooLarge)?;
    let zlib_len_u32 = u32::try_from(zlib_len).map_err(|_| Error::TooLarge)?;

    let emit_iccp = include_iccp && is_display_p3(&cicp);
    let icc = if emit_iccp {
        display_p3_icc_v4()
    } else {
        Vec::new()
    };
    let exact = 8 + 25 + 16 + (if emit_iccp { icc.len() + 35 } else { 0 }) + 12 + zlib_len + 12;
    let mut out = Vec::new();
    out.try_reserve_exact(exact).map_err(|_| Error::TooLarge)?;

    // PNG signature
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    // IHDR
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = if channels == 3 { 2 } else { 6 }; // Truecolor / Truecolor+Alpha
    push_png_chunk(&mut out, b"IHDR", &ihdr);

    // cICP (PNG 3rd Edition): must precede PLTE/IDAT and takes precedence over iCCP
    let cicp_bytes = [
        cicp.color_primaries,
        cicp.transfer_characteristics,
        cicp.matrix_coefficients,
        u8::from(cicp.full_range),
    ];
    push_png_chunk(&mut out, b"cICP", &cicp_bytes);

    // Optional iCCP fallback (uncompressed zlib stored block wrapping 492-byte Display-P3 ICC)
    if emit_iccp {
        let mut iccp = Vec::with_capacity(12 + 2 + 5 + icc.len() + 4);
        iccp.extend_from_slice(b"Display P3\0");
        iccp.push(0); // compression method 0 = zlib
        iccp.extend_from_slice(&[0x78, 0x01]); // zlib header (no preset dict, fastest)
        let ilen = icc.len() as u16;
        iccp.push(0x01); // BFINAL=1, BTYPE=00 (stored)
        iccp.extend_from_slice(&ilen.to_le_bytes());
        iccp.extend_from_slice(&(!ilen).to_le_bytes());
        iccp.extend_from_slice(&icc);
        let (s1, s2) = adler32_update((1, 0), &icc);
        iccp.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
        push_png_chunk(&mut out, b"iCCP", &iccp);
    }

    // IDAT: stream scanlines directly into stored DEFLATE blocks without allocating
    // a second uncompressed raw_total copy!
    out.extend_from_slice(&zlib_len_u32.to_be_bytes());
    let idat_tag_start = out.len();
    out.extend_from_slice(b"IDAT");
    out.extend_from_slice(&[0x78, 0x01]); // zlib CMF/FLG (CINFO=7, CM=8, FCHECK valid)

    let mut adler = (1u32, 0u32);
    let mut remaining = raw_total;
    let mut row_idx = 0usize;
    let mut row_off = 0usize; // 0 means filter byte; 1..=row_bytes means pixel data

    while remaining > 0 {
        let blk = remaining.min(65_535);
        remaining -= blk;
        let bfinal = u8::from(remaining == 0);
        let blk_u16 = blk as u16;
        out.push(bfinal);
        out.extend_from_slice(&blk_u16.to_le_bytes());
        out.extend_from_slice(&(!blk_u16).to_le_bytes());

        let mut left = blk;
        while left > 0 {
            if row_off == 0 {
                out.push(0); // PNG filter byte 0 = None
                adler.1 = (adler.1 + adler.0) % 65_521;
                row_off = 1;
                left -= 1;
                if left == 0 {
                    break;
                }
            }
            let px_off = row_off - 1;
            let take = left.min(row_bytes - px_off);
            let slice = &pixels[row_idx * row_bytes + px_off..row_idx * row_bytes + px_off + take];
            out.extend_from_slice(slice);
            adler = adler32_update(adler, slice);
            row_off += take;
            left -= take;
            if row_off == raw_row {
                row_off = 0;
                row_idx += 1;
            }
        }
    }

    let adler_val = (adler.1 << 16) | adler.0;
    out.extend_from_slice(&adler_val.to_be_bytes());
    let idat_crc = crc32_update(0, &out[idat_tag_start..]);
    out.extend_from_slice(&idat_crc.to_be_bytes());

    // IEND
    push_png_chunk(&mut out, b"IEND", &[]);
    debug_assert_eq!(out.len(), exact);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_and_adler32_match_known_vectors() {
        assert_eq!(crc32_update(0, b"123456789"), 0xCBF4_3926);
        let (s1, s2) = adler32_update((1, 0), b"123456789");
        assert_eq!((s2 << 16) | s1, 0x091E_01DE);
        let (s1, s2) = adler32_update((1, 0), b"Wikipedia");
        assert_eq!((s2 << 16) | s1, 0x11E6_0398);
    }

    #[test]
    fn display_p3_icc_header_and_d50_sum_exact() {
        let icc = display_p3_icc_v4();
        assert_eq!(icc.len(), 492);
        assert_eq!(&icc[36..40], b"acsp");
        assert_eq!(u32::from_be_bytes(icc[0..4].try_into().unwrap()), 492);
        let rx = i32::from_be_bytes(icc[408..412].try_into().unwrap());
        let ry = i32::from_be_bytes(icc[412..416].try_into().unwrap());
        let rz = i32::from_be_bytes(icc[416..420].try_into().unwrap());
        let gx = i32::from_be_bytes(icc[428..432].try_into().unwrap());
        let gy = i32::from_be_bytes(icc[432..436].try_into().unwrap());
        let gz = i32::from_be_bytes(icc[436..440].try_into().unwrap());
        let bx = i32::from_be_bytes(icc[448..452].try_into().unwrap());
        let by = i32::from_be_bytes(icc[452..456].try_into().unwrap());
        let bz = i32::from_be_bytes(icc[456..460].try_into().unwrap());
        assert_eq!(
            (rx + gx + bx, ry + gy + by, rz + gz + bz),
            (0xF6D6, 0x10000, 0xD32D)
        );
    }

    #[test]
    fn bmp_odd_widths_and_v5_icc_offsets_exact() {
        for w in [1u32, 2, 3, 5, 17, 257] {
            let h = 5u32;
            let rgb: Vec<u8> = (0..(w * h * 3) as usize).map(|i| (i * 37) as u8).collect();
            let bmp24 = encode_bmp(&rgb, w, h, 3, false, false).unwrap();
            let row = (w as usize * 3 + 3) & !3;
            assert_eq!(bmp24.len(), 54 + row * h as usize);
            assert_eq!(&bmp24[0..2], b"BM");
            // Check bottom-up pixel (0, 0) lands at last row
            let bottom = 54 + (h as usize - 1) * row;
            assert_eq!(
                [bmp24[bottom + 2], bmp24[bottom + 1], bmp24[bottom]],
                rgb[0..3]
            );

            let bmp_p3 = encode_bmp(&rgb, w, h, 3, true, true).unwrap();
            assert_eq!(bmp_p3.len(), 14 + 124 + row * h as usize + 492);
            let prof_off = u32::from_le_bytes(bmp_p3[126..130].try_into().unwrap()) as usize;
            let prof_len = u32::from_le_bytes(bmp_p3[130..134].try_into().unwrap()) as usize;
            assert_eq!(prof_len, 492);
            assert_eq!(
                &bmp_p3[14 + prof_off..14 + prof_off + prof_len],
                &display_p3_icc_v4()[..]
            );
        }
    }

    #[test]
    fn store_png_multiblock_and_chunks_valid() {
        // 300x100 RGB = 90,100 raw bytes -> spans 2 stored DEFLATE blocks!
        let (w, h) = (300u32, 100u32);
        let rgb: Vec<u8> = (0..(w * h * 3) as usize)
            .map(|i| (i * 53 + 7) as u8)
            .collect();
        let p3 = Cicp {
            color_primaries: 12,
            transfer_characteristics: 13,
            matrix_coefficients: 0,
            full_range: true,
        };
        let png = encode_store_png(&rgb, w, h, 3, p3, true).unwrap();
        assert_eq!(&png[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);

        // Walk every chunk, verify CRC-32, verify cICP, iCCP, and reconstruct IDAT scanlines bit-exact!
        let mut pos = 8;
        let mut tags = Vec::new();
        let mut idat = Vec::new();
        while pos < png.len() {
            let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
            let tag = &png[pos + 4..pos + 8];
            let body = &png[pos + 8..pos + 8 + len];
            let stored_crc =
                u32::from_be_bytes(png[pos + 8 + len..pos + 12 + len].try_into().unwrap());
            let calc_crc = crc32_update(crc32_update(0, tag), body);
            assert_eq!(
                stored_crc,
                calc_crc,
                "bad CRC on {:?}",
                core::str::from_utf8(tag)
            );
            tags.push(core::str::from_utf8(tag).unwrap().to_string());
            if tag == b"cICP" {
                assert_eq!(body, &[12, 13, 0, 1]);
            }
            if tag == b"IDAT" {
                idat.extend_from_slice(body);
            }
            pos += 12 + len;
        }
        assert_eq!(tags, vec!["IHDR", "cICP", "iCCP", "IDAT", "IEND"]);

        // Decode stored zlib stream and verify every single pixel bit-exact!
        assert_eq!(&idat[0..2], &[0x78, 0x01]);
        let mut zp = 2;
        let mut raw = Vec::new();
        loop {
            let bfinal = idat[zp];
            let blen = u16::from_le_bytes(idat[zp + 1..zp + 3].try_into().unwrap()) as usize;
            let bnlen = u16::from_le_bytes(idat[zp + 3..zp + 5].try_into().unwrap()) as usize;
            assert_eq!(blen ^ bnlen, 0xFFFF);
            zp += 5;
            raw.extend_from_slice(&idat[zp..zp + blen]);
            zp += blen;
            if bfinal & 1 != 0 {
                break;
            }
        }
        let stored_adler = u32::from_be_bytes(idat[zp..zp + 4].try_into().unwrap());
        let (s1, s2) = adler32_update((1, 0), &raw);
        assert_eq!(stored_adler, (s2 << 16) | s1);
        assert_eq!(zp + 4, idat.len());

        let mut recovered = Vec::with_capacity(rgb.len());
        for row in raw.chunks_exact(w as usize * 3 + 1) {
            assert_eq!(row[0], 0);
            recovered.extend_from_slice(&row[1..]);
        }
        assert_eq!(recovered, rgb);
    }
}
