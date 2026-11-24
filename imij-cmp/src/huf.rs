//! Safe-Rust interleaved Huffman decoder prototype, measured on imij's
//! symbol-stream dumps against zstd's HUF (`benchmarks/hufbench_*.c`).
//!
//! Format (bench-only): 64 KB blocks; per block an order-0 canonical code
//! limited to 11 bits and S independent LSB-first bitstreams, stream i
//! holding the i-th of S equal slices of the block's symbols. Codes are
//! bit-reversed so the decoder indexes its 2048-entry table with the low
//! 11 bits of a 64-bit container (Giesen's branchless refill: 56+ valid
//! bits after a refill, so 5 symbols per refill).

// The decode loops index by (symbol k, stream i) on purpose: that order is
// the measured interleave, so the range-loop and chunk lints stay off here.
#![allow(clippy::needless_range_loop, clippy::chunks_exact_to_as_chunks)]

const TB: u32 = 11;
const TS: usize = 1 << TB;
const BLOCK: usize = 1 << 16;
/// Symbols decoded per stream between refills (5 * 11 <= 56).
const PER_REFILL: usize = 5;

/// Code lengths for `freq`, at most `TB` bits.
fn lengths(freq: &[u64; 256]) -> [u8; 256] {
    let mut len = [0u8; 256];
    let mut syms: Vec<usize> = (0..256).filter(|&s| freq[s] > 0).collect();
    match syms.len() {
        0 => return len,
        1 => {
            len[syms[0]] = 1;
            return len;
        }
        _ => {}
    }
    syms.sort_by_key(|&s| (freq[s], s));
    // Two-queue Huffman over sorted leaves; parent links give depths.
    let nl = syms.len();
    let mut w: Vec<u64> = syms.iter().map(|&s| freq[s]).collect();
    let mut parent = vec![0usize; 2 * nl - 1];
    let (mut li, mut ii) = (0usize, nl);
    let pick = |w: &Vec<u64>, li: &mut usize, ii: &mut usize, next: usize| -> usize {
        if *li < nl && (*ii >= next || w[*li] <= w[*ii]) {
            *li += 1;
            *li - 1
        } else {
            *ii += 1;
            *ii - 1
        }
    };
    for k in nl..2 * nl - 1 {
        let a = pick(&w, &mut li, &mut ii, k);
        let b = pick(&w, &mut li, &mut ii, k);
        w.push(w[a] + w[b]);
        parent[a] = k;
        parent[b] = k;
    }
    let root = 2 * nl - 2;
    let mut depth = vec![0u32; 2 * nl - 1];
    for k in (0..root).rev() {
        depth[k] = depth[parent[k]] + 1;
    }
    for (i, &s) in syms.iter().enumerate() {
        len[s] = depth[i].min(TB) as u8;
    }
    // Repair Kraft: lengthen the longest codes below TB (cheapest first).
    let kraft = |len: &[u8; 256]| -> u64 {
        len.iter()
            .filter(|&&l| l > 0)
            .map(|&l| 1u64 << (TB - l as u32))
            .sum()
    };
    while kraft(&len) > TS as u64 {
        let s = (0..256)
            .filter(|&s| len[s] > 0 && (len[s] as u32) < TB)
            .max_by_key(|&s| (len[s], u64::MAX - freq[s]))
            .unwrap();
        len[s] += 1;
    }
    // Spend leftover code space on the most frequent symbols.
    loop {
        let k = kraft(&len);
        let Some(s) = (0..256)
            .filter(|&s| len[s] > 1 && k + (1u64 << (TB - len[s] as u32)) <= TS as u64)
            .max_by_key(|&s| freq[s])
        else {
            break;
        };
        len[s] -= 1;
    }
    len
}

/// Canonical codes, bit-reversed for LSB-first emission.
fn codes(len: &[u8; 256]) -> [u32; 256] {
    let mut order: Vec<usize> = (0..256).filter(|&s| len[s] > 0).collect();
    order.sort_by_key(|&s| (len[s], s));
    let mut code = [0u32; 256];
    let (mut c, mut prev) = (0u32, 0u8);
    for &s in &order {
        c <<= len[s] - prev;
        prev = len[s];
        code[s] = c.reverse_bits() >> (32 - len[s] as u32);
        c += 1;
    }
    code
}

/// Decode table: `symbol | length << 8`.
fn table(len: &[u8; 256]) -> Box<[u16; TS]> {
    let code = codes(len);
    let mut t: Box<[u16; TS]> = vec![0u16; TS].into_boxed_slice().try_into().unwrap();
    for s in 0..256 {
        let l = len[s] as u32;
        if l == 0 {
            continue;
        }
        for k in 0..1usize << (TB - l) {
            t[code[s] as usize | k << l] = s as u16 | (l as u16) << 8;
        }
    }
    t
}

struct Block {
    n: usize,
    table: Box<[u16; TS]>,
    /// Same table as `length | symbol << 8` (the sentinel loop shifts by
    /// the entry directly).
    table2: Box<[u16; TS]>,
    /// Each stream padded with 16 zero bytes (not counted in the size): the
    /// container runs up to 8 bytes ahead and each refill loads 8.
    streams: Vec<Vec<u8>>,
    /// Payload bytes plus a header estimate (4 bits per length up to the
    /// last used symbol, 3 bytes per stream size).
    bytes: usize,
}

fn encode_block(syms: &[u8], s: usize) -> Block {
    let mut freq = [0u64; 256];
    for &b in syms {
        freq[b as usize] += 1;
    }
    let len = lengths(&freq);
    let code = codes(&len);
    let q = syms.len().div_ceil(s);
    let mut streams = Vec::new();
    let mut bytes = 0;
    for part in syms.chunks(q.max(1)) {
        let (mut out, mut acc, mut n) = (Vec::new(), 0u64, 0u32);
        for &b in part {
            acc |= (code[b as usize] as u64) << n;
            n += len[b as usize] as u32;
            while n >= 8 {
                out.push(acc as u8);
                acc >>= 8;
                n -= 8;
            }
        }
        if n > 0 {
            out.push(acc as u8);
        }
        bytes += out.len() + 3;
        out.extend_from_slice(&[0; 16]);
        streams.push(out);
    }
    while streams.len() < s {
        streams.push(vec![0; 16]);
    }
    let last = (0..256).rev().find(|&i| len[i] > 0).unwrap_or(0);
    bytes += (last + 1).div_ceil(2);
    let t = table(&len);
    let table2: Box<[u16; TS]> = t
        .iter()
        .map(|&e| (e >> 8) | (e & 0xff) << 8)
        .collect::<Vec<u16>>()
        .into_boxed_slice()
        .try_into()
        .unwrap();
    Block {
        n: syms.len(),
        table: t,
        table2,
        streams,
        bytes,
    }
}

#[inline(always)]
fn refill(buf: &[u8], c: &mut u64, n: &mut u32, p: &mut usize) {
    let w = u64::from_le_bytes(buf[*p..*p + 8].try_into().unwrap());
    *c |= w << *n;
    *p += ((63 - *n) >> 3) as usize;
    *n |= 56;
}

#[inline(always)]
fn sym(t: &[u16; TS], c: &mut u64, n: &mut u32) -> u8 {
    let e = t[(*c as usize) & (TS - 1)];
    let l = (e >> 8) as u32;
    *c >>= l;
    *n -= l;
    e as u8
}

/// Decodes `S` interleaved streams into `out` (the block's symbols).
#[inline(always)]
fn decode_block<const S: usize>(t: &[u16; TS], streams: &[Vec<u8>], out: &mut [u8]) {
    let q = out.len().div_ceil(S).max(1);
    let mut c = [0u64; S];
    let mut n = [0u32; S];
    let mut p = [0usize; S];
    let bufs: [&[u8]; S] = core::array::from_fn(|i| &streams[i][..]);
    let parts: [&mut [u8]; S] = {
        let mut it = out.chunks_mut(q);
        core::array::from_fn(|_| it.next().unwrap_or(&mut []))
    };
    // Lockstep rounds while every stream has PER_REFILL symbols left (the
    // last part is the shortest).
    let rounds = parts[S - 1].len() / PER_REFILL;
    for r in 0..rounds {
        for i in 0..S {
            refill(bufs[i], &mut c[i], &mut n[i], &mut p[i]);
        }
        let mut d = [[0u8; PER_REFILL]; S];
        for k in 0..PER_REFILL {
            for i in 0..S {
                d[i][k] = sym(t, &mut c[i], &mut n[i]);
            }
        }
        let o = r * PER_REFILL;
        for i in 0..S {
            parts[i][o..o + PER_REFILL].copy_from_slice(&d[i]);
        }
    }
    // Tails.
    for i in 0..S {
        for j in rounds * PER_REFILL..parts[i].len() {
            refill(bufs[i], &mut c[i], &mut n[i], &mut p[i]);
            parts[i][j] = sym(t, &mut c[i], &mut n[i]);
        }
    }
}

/// Sentinel refill: the container holds its valid bits below a single 1
/// bit, so no per-symbol bit count is kept; the count is the sentinel's
/// position, recovered here once per refill.
#[inline(always)]
fn refill_s(buf: &[u8], c: &mut u64, p: &mut usize) {
    let n = 63 - c.leading_zeros();
    let w = u64::from_le_bytes(buf[*p..*p + 8].try_into().unwrap());
    let n2 = n | 56;
    let v = (*c ^ (1u64 << n)) | (w << n);
    *c = (v & ((1u64 << n2) - 1)) | (1u64 << n2);
    *p += ((63 - n) >> 3) as usize;
}

#[inline(always)]
fn sym_s(t: &[u16; TS], c: &mut u64) -> u8 {
    let e = t[(*c as usize) & (TS - 1)];
    // The low 6 bits of the entry are the length.
    *c = c.wrapping_shr(e as u32);
    (e >> 8) as u8
}

/// `decode_block` with the sentinel container and direct output stores.
#[inline(always)]
fn decode_block_s<const S: usize>(t: &[u16; TS], streams: &[Vec<u8>], out: &mut [u8]) {
    let q = out.len().div_ceil(S).max(1);
    let mut c = [1u64; S];
    let mut p = [0usize; S];
    let bufs: [&[u8]; S] = core::array::from_fn(|i| &streams[i][..]);
    let mut parts: [&mut [u8]; S] = {
        let mut it = out.chunks_mut(q);
        core::array::from_fn(|_| it.next().unwrap_or(&mut []))
    };
    let m = parts[S - 1].len() / PER_REFILL * PER_REFILL;
    {
        let mut its = parts
            .each_mut()
            .map(|x| x[..m].chunks_exact_mut(PER_REFILL));
        for _ in 0..m / PER_REFILL {
            for i in 0..S {
                refill_s(bufs[i], &mut c[i], &mut p[i]);
            }
            let d: [&mut [u8; PER_REFILL]; S] =
                core::array::from_fn(|i| its[i].next().unwrap().try_into().unwrap());
            for k in 0..PER_REFILL {
                for i in 0..S {
                    d[i][k] = sym_s(t, &mut c[i]);
                }
            }
        }
    }
    for i in 0..S {
        for j in m..parts[i].len() {
            refill_s(bufs[i], &mut c[i], &mut p[i]);
            parts[i][j] = sym_s(t, &mut c[i]);
        }
    }
}

#[inline(always)]
fn decode_all<const S: usize>(blocks: &[Block], out: &mut [u8]) {
    let sentinel = SENTINEL.load(core::sync::atomic::Ordering::Relaxed);
    for (b, o) in blocks.iter().zip(out.chunks_mut(BLOCK)) {
        if sentinel {
            decode_block_s::<S>(&b.table2, &b.streams, &mut o[..b.n]);
        } else {
            decode_block::<S>(&b.table, &b.streams, &mut o[..b.n]);
        }
    }
}

static SENTINEL: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[inline(never)]
fn decode_base<const S: usize>(blocks: &[Block], out: &mut [u8]) {
    decode_all::<S>(blocks, out)
}

/// The same loop compiled with BMI2 (`shrx`: shifts without the `cl` register).
#[archmage::arcane]
fn decode_v3_4(_t: archmage::X64V3Token, blocks: &[Block], out: &mut [u8]) {
    decode_all::<4>(blocks, out)
}
#[archmage::arcane]
fn decode_v3_6(_t: archmage::X64V3Token, blocks: &[Block], out: &mut [u8]) {
    decode_all::<6>(blocks, out)
}
#[archmage::arcane]
fn decode_v3_8(_t: archmage::X64V3Token, blocks: &[Block], out: &mut [u8]) {
    decode_all::<8>(blocks, out)
}

fn decode_dispatch<const S: usize>(v3: bool, blocks: &[Block], out: &mut [u8]) {
    use archmage::SimdToken;
    match (v3, archmage::X64V3Token::summon()) {
        (true, Some(t)) if S == 4 => decode_v3_4(t, blocks, out),
        (true, Some(t)) if S == 6 => decode_v3_6(t, blocks, out),
        (true, Some(t)) if S == 8 => decode_v3_8(t, blocks, out),
        _ => decode_base::<S>(blocks, out),
    }
}

fn bench<const S: usize>(data: &[u8], iters: usize) -> (f64, f64) {
    let blocks: Vec<Block> = data.chunks(BLOCK).map(|c| encode_block(c, S)).collect();
    let bytes: usize = blocks.iter().map(|b| b.bytes).sum();
    let v3 = std::env::var("HUF_BASE").is_err();
    let mut out = vec![0u8; data.len()];
    decode_dispatch::<S>(v3, &blocks, &mut out);
    assert!(out == data, "huf roundtrip mismatch (S = {S})");
    let best = (0..iters)
        .map(|_| {
            let t = std::time::Instant::now();
            decode_dispatch::<S>(v3, &blocks, &mut out);
            std::hint::black_box(&out);
            t.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min);
    (bytes as f64 * 8.0 / data.len() as f64, best)
}

pub fn cmd(files: &[String], iters: usize) {
    for sentinel in [false, true] {
        SENTINEL.store(sentinel, core::sync::atomic::Ordering::Relaxed);
        println!(
            "{} loop{}",
            if sentinel { "sentinel" } else { "counter" },
            if std::env::var("HUF_BASE").is_ok() {
                " (x86-64 baseline)"
            } else {
                " (BMI2)"
            }
        );
        cmd1(files, iters);
    }
}

fn cmd1(files: &[String], iters: usize) {
    println!(
        "{:<22} {:>10} {:>9} | {:>14} {:>14} {:>14}",
        "stream", "symbols", "bits/sym", "S=4 MB/s ns", "S=6 MB/s ns", "S=8 MB/s ns"
    );
    for f in files {
        let data = std::fs::read(f).unwrap();
        let name = std::path::Path::new(f)
            .file_stem()
            .unwrap()
            .to_string_lossy();
        let r4 = bench::<4>(&data, iters);
        let r6 = bench::<6>(&data, iters);
        let r8 = bench::<8>(&data, iters);
        let fmt = |(_, s): (f64, f64)| {
            format!(
                "{:>7.0} {:>6.3}",
                data.len() as f64 / s / 1e6,
                s * 1e9 / data.len() as f64
            )
        };
        println!(
            "{name:<22} {:>10} {:>9.3} | {:>14} {:>14} {:>14}",
            data.len(),
            r4.0,
            fmt(r4),
            fmt(r6),
            fmt(r8)
        );
    }
}
