//! Baseline JPEG files from IMI's quantized coefficients (AQ + trellis
//! study, 2026-10-09): the same integer symbols packed by JPEG's Huffman
//! coder, so JPEG XL's lossless JPEG transcode can code them too and the
//! three coders compare on identical symbols.
//!
//! `coef-jpeg FILE.imi AC.jpg ZERO.jpg` writes a baseline sequential JPEG of
//! the file's AC coefficients (DC zero; three 1x1 components; quantization
//! tables of ones; Huffman tables optimized as libjpeg's
//! `jpeg_gen_optimal_table`, luma tables for component 1, shared chroma
//! tables for 2 and 3) and the same file with every AC zero, and prints
//! `{"scan_ac", "scan_zero", "jpeg_ac", "jpeg_zero", "clamped", "imij_ac",
//! "imij_dc", "imij_bytes"}`. AC values beyond baseline's +-1023 are clamped
//! and counted.

use crate::rd::Result;

/// Natural index of JPEG zigzag position k.
pub(crate) const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

fn size(v: i32) -> u32 {
    32 - v.unsigned_abs().leading_zeros()
}

/// libjpeg's jpeg_gen_optimal_table: (bits[1..=16], huffval).
fn optimal_table(freq_in: &[u64; 256]) -> ([u8; 17], Vec<u8>) {
    let mut freq = [0u64; 257];
    freq[..256].copy_from_slice(freq_in);
    freq[256] = 1;
    let mut codesize = [0usize; 257];
    let mut others = [-1i32; 257];
    loop {
        let (mut c1, mut c2) = (-1i32, -1i32);
        let mut v = u64::MAX;
        for (i, &f) in freq.iter().enumerate() {
            if f > 0 && f <= v {
                v = f;
                c1 = i as i32;
            }
        }
        v = u64::MAX;
        for (i, &f) in freq.iter().enumerate() {
            if f > 0 && f <= v && i as i32 != c1 {
                v = f;
                c2 = i as i32;
            }
        }
        if c2 < 0 {
            break;
        }
        let (a, b) = (c1 as usize, c2 as usize);
        freq[a] += freq[b];
        freq[b] = 0;
        let mut x = a;
        codesize[x] += 1;
        while others[x] >= 0 {
            x = others[x] as usize;
            codesize[x] += 1;
        }
        others[x] = b as i32;
        let mut y = b;
        codesize[y] += 1;
        while others[y] >= 0 {
            y = others[y] as usize;
            codesize[y] += 1;
        }
    }
    let mut bits = [0i64; 33];
    for &c in &codesize {
        if c > 0 {
            bits[c] += 1;
        }
    }
    let mut i = 32;
    while i > 16 {
        while bits[i] > 0 {
            let mut j = i - 2;
            while bits[j] == 0 {
                j -= 1;
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
        }
        i -= 1;
    }
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1;
    let mut out = [0u8; 17];
    for k in 1..=16 {
        out[k] = bits[k] as u8;
    }
    let mut huffval = Vec::new();
    for len in 1..=32 {
        for (s, &c) in codesize.iter().enumerate().take(256) {
            if c == len {
                huffval.push(s as u8);
            }
        }
    }
    (out, huffval)
}

/// Canonical codes: symbol -> (code, length).
fn codes(bits: &[u8; 17], huffval: &[u8]) -> [(u32, u32); 256] {
    let mut table = [(0u32, 0u32); 256];
    let (mut code, mut k) = (0u32, 0usize);
    for len in 1..=16u32 {
        for _ in 0..bits[len as usize] {
            table[huffval[k] as usize] = (code, len);
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    table
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u32, len: u32) {
        self.acc = (self.acc << len) | code as u64;
        self.n += len;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0);
            }
            self.n -= 8;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
        self.out
    }
}

/// Visits a block's AC symbols: (symbol, value bits, value length).
fn ac_symbols(blk: &[i32], clamped: &mut usize, mut f: impl FnMut(u8, u32, u32)) {
    let mut run = 0u32;
    let last = (1..64).rev().find(|&k| blk[ZIGZAG[k]] != 0).unwrap_or(0);
    for k in 1..=last {
        let mut v = blk[ZIGZAG[k]];
        if v == 0 {
            run += 1;
            continue;
        }
        if v.abs() > 1023 {
            *clamped += 1;
            v = v.clamp(-1023, 1023);
        }
        while run >= 16 {
            f(0xF0, 0, 0);
            run -= 16;
        }
        let s = size(v);
        let bits = if v < 0 {
            (v - 1) as u32 & ((1 << s) - 1)
        } else {
            v as u32
        };
        f(((run << 4) | s) as u8, bits, s);
        run = 0;
    }
    if last < 63 {
        f(0x00, 0, 0);
    }
}

/// A baseline JPEG of `bw` x `bh` blocks (AC zero unless `with_ac`);
/// returns (file, entropy-coded bytes, clamped AC values, clamped DCs).
fn write(bw: usize, bh: usize, q: &[i32], with_ac: bool) -> (Vec<u8>, usize, usize, usize) {
    let zero = [0i32; 64];
    let block = |b: usize, ch: usize| -> &[i32] {
        if with_ac {
            &q[(b * 3 + ch) * 64..][..64]
        } else {
            &zero
        }
    };
    let mut clamped_dc = 0usize;
    // DC differences per component in block order (one MCU per block).
    let mut prev = [0i32; 3];
    let diffs: Vec<i32> = (0..bw * bh * 3)
        .map(|i| {
            let v = q[i * 64];
            if v.abs() > 1023 {
                clamped_dc += 1;
            }
            let v = v.clamp(-1023, 1023);
            let d = v - prev[i % 3];
            prev[i % 3] = v;
            d
        })
        .collect();
    let mut dc_freq = [[0u64; 256]; 2];
    let mut ac_freq = [[0u64; 256]; 2];
    let mut clamped = 0usize;
    for b in 0..bw * bh {
        for ch in 0..3 {
            let t = (ch > 0) as usize;
            dc_freq[t][size(diffs[b * 3 + ch]) as usize] += 1;
            ac_symbols(block(b, ch), &mut clamped, |s, _, _| {
                ac_freq[t][s as usize] += 1
            });
        }
    }
    let tables: Vec<([u8; 17], Vec<u8>)> = [dc_freq[0], dc_freq[1], ac_freq[0], ac_freq[1]]
        .iter()
        .map(optimal_table)
        .collect();
    let dc_codes = [
        codes(&tables[0].0, &tables[0].1),
        codes(&tables[1].0, &tables[1].1),
    ];
    let ac_codes = [
        codes(&tables[2].0, &tables[2].1),
        codes(&tables[3].0, &tables[3].1),
    ];
    let mut out = vec![0xFF, 0xD8];
    let seg = |out: &mut Vec<u8>, marker: u8, data: &[u8]| {
        out.extend_from_slice(&[0xFF, marker]);
        out.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(data);
    };
    let mut dqt = Vec::new();
    for t in 0..2u8 {
        dqt.push(t);
        dqt.extend_from_slice(&[1u8; 64]);
    }
    seg(&mut out, 0xDB, &dqt);
    let (w, h) = ((bw * 8) as u16, (bh * 8) as u16);
    let mut sof = vec![8u8];
    sof.extend_from_slice(&h.to_be_bytes());
    sof.extend_from_slice(&w.to_be_bytes());
    sof.push(3);
    for (id, tq) in [(1u8, 0u8), (2, 1), (3, 1)] {
        sof.extend_from_slice(&[id, 0x11, tq]);
    }
    seg(&mut out, 0xC0, &sof);
    let mut dht = Vec::new();
    for (i, (bits, vals)) in tables.iter().enumerate() {
        let class = if i < 2 { 0u8 } else { 1u8 };
        dht.push((class << 4) | (i % 2) as u8);
        dht.extend_from_slice(&bits[1..]);
        dht.extend_from_slice(vals);
    }
    seg(&mut out, 0xC4, &dht);
    seg(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    let mut bitw = Bits {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    let mut dummy = 0usize;
    for b in 0..bw * bh {
        for ch in 0..3 {
            let t = (ch > 0) as usize;
            let d = diffs[b * 3 + ch];
            let s = size(d);
            let (c, l) = dc_codes[t][s as usize];
            bitw.put(c, l);
            if s > 0 {
                let bits = if d < 0 {
                    (d - 1) as u32 & ((1 << s) - 1)
                } else {
                    d as u32
                };
                bitw.put(bits, s);
            }
            ac_symbols(block(b, ch), &mut dummy, |s, v, n| {
                let (c, l) = ac_codes[t][s as usize];
                bitw.put(c, l);
                if n > 0 {
                    bitw.put(v, n);
                }
            });
        }
    }
    let scan = bitw.finish();
    let scan_len = scan.len();
    out.extend_from_slice(&scan);
    out.extend_from_slice(&[0xFF, 0xD9]);
    (out, scan_len, clamped, clamped_dc)
}

pub(crate) fn command(a: &[String]) -> Result<()> {
    if a.len() != 5 {
        return Err("coef-jpeg FILE.imij FULL.jpg DC.jpg".into());
    }
    let data = std::fs::read(&a[2])?;
    let (bw, bh, q, ac, dc) = imij::lab::lossy_quantized(&data)?;
    let (full, scan_full, clamped_ac, clamped_dc) = write(bw, bh, &q, true);
    let (dconly, scan_dc, _, _) = write(bw, bh, &q, false);
    std::fs::write(&a[3], &full)?;
    std::fs::write(&a[4], &dconly)?;
    println!(
        "{}",
        serde_json::json!({"scan_full": scan_full, "scan_dc": scan_dc, "jpeg_full": full.len(),
            "jpeg_dc": dconly.len(), "clamped_ac": clamped_ac, "clamped_dc": clamped_dc,
            "imij_ac": ac, "imij_dc": dc, "imij_bytes": data.len(), "blocks": bw * bh})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimal_tables_are_prefix_codes_within_16_bits() {
        let mut f = [0u64; 256];
        for (i, v) in f.iter_mut().enumerate() {
            *v = if i % 3 == 0 {
                0
            } else {
                1 + (i as u64 * 7919) % 1000
            };
        }
        let (bits, vals) = optimal_table(&f);
        let n: usize = bits[1..].iter().map(|&b| b as usize).sum();
        assert_eq!(n, vals.len());
        assert_eq!(n, f.iter().filter(|&&v| v > 0).count());
        // Kraft sum < 1 (the all-ones code stays unused).
        let k: f64 = (1..=16).map(|l| bits[l] as f64 / (1u64 << l) as f64).sum();
        assert!(k < 1.0, "{k}");
    }

    /// zenjpeg decodes the writer's files to exactly the coefficients they
    /// pack (zigzag order; values clamped beyond +-1023), DC-only files too.
    #[test]
    fn written_jpeg_decodes_to_its_coefficients() {
        let (bw, bh) = (5usize, 3usize);
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut q = vec![0i32; bw * bh * 3 * 64];
        for (i, v) in q.iter_mut().enumerate() {
            let k = i % 64;
            if k == 0 {
                *v = (rnd() % 2047) as i32 - 1023 + if i == 128 { 2000 } else { 0 };
                continue;
            }
            *v = match rnd() % 9 {
                0 => (rnd() % 2047) as i32 - 1023,
                1 | 2 => (rnd() % 7) as i32 - 3,
                3 if i % 640 == 7 => 1500,
                _ => 0,
            };
        }
        // A block with one coefficient at the end (a run past 16).
        for k in 1..64 {
            q[64 + k] = 0;
        }
        q[64 + 63] = -2;
        let (jpeg, _, clamped, clamped_dc) = write(bw, bh, &q, true);
        assert!(clamped > 0);
        assert!(clamped_dc >= 1);
        let dec = zenjpeg::decoder::DecodeConfig::new()
            .decode_coefficients(&jpeg, enough::Unstoppable)
            .unwrap();
        assert_eq!(dec.components.len(), 3);
        for (ch, comp) in dec.components.iter().enumerate() {
            assert_eq!((comp.blocks_wide, comp.blocks_high), (bw, bh));
            for b in 0..bw * bh {
                let dc = q[(b * 3 + ch) * 64].clamp(-1023, 1023);
                assert_eq!(comp.coeffs[b * 64] as i32, dc, "DC ch {ch} block {b}");
                for k in 1..64 {
                    let want = q[(b * 3 + ch) * 64 + ZIGZAG[k]].clamp(-1023, 1023);
                    assert_eq!(
                        comp.coeffs[b * 64 + k] as i32,
                        want,
                        "ch {ch} block {b} k {k}"
                    );
                }
            }
        }
        let (dconly, _, _, _) = write(bw, bh, &q, false);
        let z = zenjpeg::decoder::DecodeConfig::new()
            .decode_coefficients(&dconly, enough::Unstoppable)
            .unwrap();
        for (ch, comp) in z.components.iter().enumerate() {
            for b in 0..bw * bh {
                let dc = q[(b * 3 + ch) * 64].clamp(-1023, 1023);
                assert_eq!(comp.coeffs[b * 64] as i32, dc);
                assert!(comp.coeffs[b * 64 + 1..b * 64 + 64].iter().all(|&v| v == 0));
            }
        }
    }
}
