//! Encoder costs of a bank's seeds, computed at compile time from the blob:
//! for every seed and symbol, the bits a symbol costs under that seed in
//! sixteenths of a bit (`ABSENT`: the seed cannot code it). The encoder ranks
//! seeds with these; the seed it picks is then decoded exactly as the decoder
//! does. The const functions repeat the table grammar of `rans.rs` /
//! `lossy/huf.rs` (tests check them against those functions).

/// Cost of a symbol a seed does not code.
pub(super) const ABSENT: u8 = 255;

const M: u32 = crate::rans::M;

/// Little-endian u32 at byte `at`.
const fn rd(b: &[u8], at: usize) -> usize {
    (b[at] as usize)
        | (b[at + 1] as usize) << 8
        | (b[at + 2] as usize) << 16
        | (b[at + 3] as usize) << 24
}

/// Alphabet of a lossless token scheme (`TokenCfg::v1(k, m, depth)`).
const fn lossless_alphabet(tc: [u8; 3]) -> usize {
    let (k, m, depth) = (tc[0] as u64, tc[1] as u64, tc[2] as u64);
    let mut kv = if k > m { k } else { m };
    if kv > depth {
        kv = depth;
    }
    let value = if kv >= depth {
        1u64 << depth
    } else {
        (1u64 << kv) + (depth - kv) * (1u64 << m)
    };
    let rk = if kv > m { kv } else { m };
    let runs = (1u64 << rk) + (crate::tokens::MAX_RUN_BITS as u64 - rk) * (1u64 << m);
    (value + runs) as usize
}

/// Alphabet a kind's tables are read with.
const fn alphabet(kind: u8, tc: [u8; 3]) -> usize {
    match kind {
        4 => crate::lossy::huf::CNT_ALPHA,
        5 => crate::lossy::huf::WID_ALPHA,
        6 => lossless_alphabet(tc),
        _ => crate::lossy::coef::ALPHA,
    }
}

/// Symbols per seed in the cost table (EOB tables code symbols 0..64 only).
pub(super) const fn width(kind: u8, tc: [u8; 3]) -> usize {
    match kind {
        0 => 64,
        _ => alphabet(kind, tc),
    }
}

/// Cost table length for the blob `b`.
pub(super) const fn len(b: &[u8]) -> usize {
    let (mut e, mut n) = (0, 0);
    while e < b[6] as usize {
        let d = 8 + 12 * e;
        n += rd(b, d + 4) * width(b[d], [b[d + 1], b[d + 2], b[d + 3]]);
        e += 1;
    }
    n
}

/// LSB-first bit reader over bytes `[at, end)` of a blob, zeros past the end
/// (as `BitReader`).
struct Bits<'a> {
    b: &'a [u8],
    at: usize,
    end: usize,
    buf: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    const fn new(b: &'a [u8], at: usize, end: usize) -> Self {
        Bits {
            b,
            at,
            end,
            buf: 0,
            n: 0,
        }
    }

    /// At least 57 buffered bits.
    const fn fill(&mut self) {
        while self.n <= 56 {
            let byte = if self.at < self.end {
                self.b[self.at]
            } else {
                0
            };
            self.buf |= (byte as u64) << self.n;
            self.at += 1;
            self.n += 8;
        }
    }

    /// The next `k` <= 32 bits.
    const fn read(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        let v = (self.buf & ((1u64 << k) - 1)) as u32;
        self.buf >>= k;
        self.n -= k;
        v
    }

    /// Zero bits before the next one bit, both consumed; `None` past 24.
    const fn zeros(&mut self) -> Option<u32> {
        if self.n < 25 {
            self.fill();
        }
        let z = self.buf.trailing_zeros();
        if z > 24 {
            return None;
        }
        self.buf >>= z + 1;
        self.n -= z + 1;
        Some(z)
    }
}

/// `rans::read_weights` of the seed at bytes `[s, end)`: weights and
/// whether they decoded.
const fn read_weights(b: &[u8], s: usize, end: usize, alpha: usize) -> ([u32; 256], bool) {
    let mut wt = [0u32; 256];
    let mut r = Bits::new(b, s, end);
    let last = r.read(8) as usize;
    let prec = r.read(3) + 1;
    if last >= alpha || alpha > 256 {
        return (wt, false);
    }
    let mut prev_e = 0i64;
    let mut sym = 0;
    while sym <= last {
        // Elias gamma: z zero bits, a one bit, z more bits.
        let Some(z) = r.zeros() else {
            return (wt, false);
        };
        let g = (1u32 << z) | r.read(z);
        let zz = (g - 1) as i64;
        let d = if zz & 1 == 0 { zz / 2 } else { -(zz + 1) / 2 };
        let e = prev_e + d;
        if e < 0 || e > 32 {
            return (wt, false);
        }
        let e = e as u32;
        wt[sym] = if e == 0 {
            0
        } else if e == 1 {
            1
        } else {
            let nb = if e - 1 < prec - 1 { e - 1 } else { prec - 1 };
            ((1u32 << nb) | r.read(nb)) << (e - 1 - nb)
        };
        prev_e = e as i64;
        sym += 1;
    }
    (wt, true)
}

/// `rans::weights_to_freq` over the first `alpha` weights.
const fn freq(wt: &[u32; 256], alpha: usize) -> ([u32; 256], bool) {
    let mut f = [0u32; 256];
    let mut order = [0u16; 256];
    let (mut n, mut total, mut i) = (0usize, 0u64, 0usize);
    while i < alpha {
        if wt[i] > 0 {
            order[n] = i as u16;
            n += 1;
            total += wt[i] as u64;
        }
        i += 1;
    }
    if n == 0 || n > M as usize {
        return (f, false);
    }
    let mut sum = 0i64;
    i = 0;
    while i < alpha {
        if wt[i] > 0 {
            let v = (wt[i] as u64 * M as u64 / total) as u32;
            f[i] = if v > 1 { v } else { 1 };
            sum += f[i] as i64;
        }
        i += 1;
    }
    if sum == M as i64 {
        return (f, true);
    }
    // Largest weight first, ties by lower index (insertion sort).
    i = 1;
    while i < n {
        let x = order[i];
        let mut j = i;
        while j > 0 {
            let y = order[j - 1];
            let (wx, wy) = (wt[x as usize], wt[y as usize]);
            if wy > wx || (wy == wx && y < x) {
                break;
            }
            order[j] = y;
            j -= 1;
        }
        order[j] = x;
        i += 1;
    }
    i = 0;
    while sum < M as i64 {
        f[order[i % n] as usize] += 1;
        sum += 1;
        i += 1;
    }
    i = 0;
    let mut guard = 0;
    while sum > M as i64 {
        let s = order[i % n] as usize;
        if f[s] > 1 {
            f[s] -= 1;
            sum -= 1;
        }
        i += 1;
        guard += 1;
        if guard > 4 * M as usize * n {
            return (f, false);
        }
    }
    (f, true)
}

/// floor(log2(f) * 2^24) for f >= 1 (fixed-point squaring).
const fn log2_q24(f: u32) -> u64 {
    let e = 31 - f.leading_zeros();
    let mut x: u128 = (f as u128) << (62 - e);
    let (mut frac, mut i) = (0u64, 0);
    while i < 24 {
        x = (x * x) >> 62;
        frac <<= 1;
        if x >= 1u128 << 63 {
            x >>= 1;
            frac |= 1;
        }
        i += 1;
    }
    ((e as u64) << 24) | frac
}

/// round(16 * (10 - log2 f)): the bits frequency `f` (of 1024) costs, in
/// sixteenths.
pub(super) const fn cost16(f: u32) -> u8 {
    let v = (160i64 << 24) - 16 * log2_q24(f) as i64;
    ((v + (1 << 23)) >> 24) as u8
}

/// `cost16` of every frequency 0..=M (0 unused), evaluated once.
const COST16: [u8; M as usize + 1] = {
    let mut t = [0u8; M as usize + 1];
    let mut f = 1;
    while f <= M {
        t[f as usize] = cost16(f);
        f += 1;
    }
    t
};

/// The cost table of blob `b` (`L` = `len(b)`): kinds in directory order,
/// `width` symbols per seed.
pub(super) const fn table<const L: usize>(b: &[u8]) -> [u8; L] {
    let mut out = [ABSENT; L];
    fill(b, &mut out);
    out
}

/// Fills `out` (`len(b)` entries, all `ABSENT`) with the cost table of blob
/// `b`; [`table`] at compile time, lab banks at run time.
pub(super) const fn fill(b: &[u8], out: &mut [u8]) {
    let (mut e, mut o) = (0, 0);
    while e < b[6] as usize {
        let d = 8 + 12 * e;
        let (kind, tc) = (b[d], [b[d + 1], b[d + 2], b[d + 3]]);
        let (n, at) = (rd(b, d + 4), rd(b, d + 8));
        let (w, alpha) = (width(kind, tc), alphabet(kind, tc));
        let data = at + 4 * (n + 1);
        let mut i = 0;
        while i < n {
            let (s, end) = (data + rd(b, at + 4 * i), data + rd(b, at + 4 * (i + 1)));
            let row = o + i * w;
            if kind == 4 || kind == 5 {
                let mut r = Bits::new(b, s, end);
                let mut sym = 0;
                while sym < w {
                    let l = r.read(4);
                    if l > 0 {
                        out[row + sym] = (l * 16) as u8;
                    }
                    sym += 1;
                }
            } else {
                let (wt, ok) = read_weights(b, s, end, alpha);
                if ok {
                    let (f, ok) = freq(&wt, alpha);
                    if ok {
                        let mut sym = 0;
                        while sym < w {
                            if f[sym] > 0 {
                                out[row + sym] = COST16[f[sym] as usize];
                            }
                            sym += 1;
                        }
                    }
                }
            }
            i += 1;
        }
        o += n * w;
        e += 1;
    }
}
