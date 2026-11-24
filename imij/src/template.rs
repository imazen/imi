//! Template palette stripes (SPEC 2.14, feature `template_palettes`): a
//! palette stripe whose index plane is coded from equality contexts of the
//! ten template pixels of bilevel stripes (JBIG's three-line template,
//! ITU-T T.82) instead of predictors and tokens.
//!
//! Per pixel, phase 0 codes "differs from W" (W the left neighbor) with a
//! context of which of the other nine template pixels equal W and whether W
//! is index 0. A pixel that differs then tries N, NE, NW, WW and NN in turn
//! (phases 1-5, each a binary symbol), and an index none of them matches is
//! coded directly (the escape phase). Rows equal to the row above code
//! nothing (typical rows). Lanes, rANS states, renormalization and word
//! order (step, phase, lane) are the lossless coder's (SPEC 3).
//!
//! The decoder keeps W and WW in registers and, with three or more entries,
//! looks the next step's probabilities up off the decode's dependency chain,
//! assuming every lane repeats W (and, after a miss, that the differing lanes
//! take N): at every step where a lookup is one hardware gather, elsewhere
//! only after a step where every lane repeated W. With two entries the
//! context is a bijection of bilevel stripes' value context, so it slides
//! from step to step and looks up one step ahead like theirs.
//! Rows are step-major with two halo steps before each row and three after
//! (the third is padding for the lookahead loads); typical rows alias the
//! row above instead of copying it.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;
#[cfg(feature = "encode")]
use crate::rans::LOG_M;
use crate::rans::{self, LaneState, M, NL, RANS_L, Words};

/// Stripe layout bytes of a template palette stripe with its own entries
/// and with the payload's shared entries (SPEC 2.4).
pub(crate) const STRIPE: u8 = 0x83;
pub(crate) const SHARED: u8 = 0x84;
/// Lossless head record id of the template tables (SPEC 2).
pub(crate) const RECORD: u64 = crate::registry::HEAD_TEMPLATE;

/// Decoder bytes of the tables and their construction's weights and
/// frequencies (charged at admission).
pub(crate) const TABLE_BYTES: usize = size_of::<Table>() + 3 * 256 * 4 + 4 * 256 * 4 + 4 * 24;

/// Template pixels `(dy, dx)`; index 9 is W (the tests' reference).
#[cfg(all(test, feature = "encode"))]
const T10: [(isize, isize); 10] = [
    (-2, -1),
    (-2, 0),
    (-2, 1),
    (-1, -2),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (-1, 2),
    (0, -2),
    (0, -1),
];
/// Candidates after W, as template indices: N, NE, NW, WW, NN. (The row
/// kernel spells this order out in its `cand!` calls.)
#[cfg(feature = "encode")]
const CAND: [usize; 5] = [5, 6, 4, 8, 1];
/// Candidate contexts: rank * 8 + count.
const NBCTX: usize = 40;

/// The decoder's tables (head record 35).
pub(crate) struct Table {
    /// Two entries: `pairs[b]` holds the phase 0 probability of value
    /// context `b` (bits 0-8: the template values) with W = 0 in its low
    /// half and W = 1 in its high half.
    pairs: [u32; 512],
    /// Phase 0: the probability that the pixel equals W, per context.
    a: [u32; 1024],
    /// Candidate phases: `b[rank * 8 + count]`; padded to a power of two.
    b: [u32; 64],
    /// The direct-index table: slot -> sym | freq << 8 | bias << 19.
    dec: [u32; M as usize],
    /// Shared entries: per slot, the coded value of each entry.
    pub(crate) shared: Option<Vec<Vec<u32>>>,
}

impl Table {
    /// Parses head record 35 (SPEC 2.14) of a payload whose slots have
    /// `slot_bits` bits.
    pub(crate) fn read(rec: &[u8], slot_bits: &[u32]) -> Result<Box<Table>, Error> {
        let mut r = BitReader::new(rec);
        let b = 4 + r.read_refill(3);
        let mut get = |out: &mut [u32]| -> Result<(), Error> {
            for p in out {
                *p = M / 2;
                if r.read_refill(1) == 1 {
                    let v = r.read_refill(b);
                    *p = if b == 11 { v } else { v << (10 - b) };
                    if *p > M {
                        return Err(Error::Corrupt("template probability above 1"));
                    }
                }
            }
            Ok(())
        };
        let mut t = Box::new(Table {
            pairs: [0; 512],
            a: [0; 1024],
            b: [M / 2; 64],
            dec: [0; M as usize],
            shared: None,
        });
        get(&mut t.a)?;
        get(&mut t.b[..NBCTX])?;
        if r.read_refill(1) == 1 {
            let (wt, _) = rans::read_weights(&mut r, 256)?;
            let freq = rans::weights_to_freq(&wt).ok_or(Error::Corrupt("empty template table"))?;
            t.dec = escape_dec(&freq);
        } else {
            // All of M on symbol 0: decodes 0 and leaves the state as is.
            let mut freq = [0u32; 256];
            freq[0] = M;
            t.dec = escape_dec(&freq);
        }
        if r.read_refill(1) == 1 {
            if slot_bits.iter().any(|&d| d > 16) {
                return Err(Error::Corrupt("shared template entries on a deep slot"));
            }
            let n = r.read_refill(8) as usize + 1;
            let mut e = vec![Vec::with_capacity(n); slot_bits.len()];
            for _ in 0..n {
                for (v, &d) in e.iter_mut().zip(slot_bits) {
                    v.push(r.read_refill(d));
                }
            }
            t.shared = Some(e);
        }
        if r.overrun() || rec.len() as u64 != r.bits_read().div_ceil(8) {
            return Err(Error::Corrupt("template table length"));
        }
        let a = &t.a;
        t.pairs = core::array::from_fn(|v| a[(v ^ 0x1FF) | 512] | a[v] << 16);
        Ok(t)
    }
}

/// The slot table of a normalized frequency table.
fn escape_dec(freq: &[u32]) -> [u32; M as usize] {
    let mut dec = [0u32; M as usize];
    let mut cum = 0u32;
    for (s, &f) in freq.iter().enumerate() {
        for slot in cum..cum + f {
            dec[slot as usize] = s as u32 | (f & 0x7FF) << 8 | (slot - cum) << 19;
        }
        cum += f;
    }
    dec
}

// ---- encoder ------------------------------------------------------------------

/// A unit of the encoder: first row, rows, first column, width and lane
/// width (as bilevel stripes').
#[cfg(feature = "encode")]
pub(crate) type Unit = (usize, usize, usize, usize, usize);

/// A unit's palette: its slot tuples (16 bits per slot, slot 0 highest) in
/// descending count order, ties in ascending tuple order, and its index
/// plane (`gw` per row); `None` above 256 tuples. `slots` are the image's
/// coded planes, `w` wide.
#[cfg(feature = "encode")]
pub(crate) fn unit_palette(slots: &[&[u32]], w: usize, u: &Unit) -> Option<(Vec<u64>, Vec<u8>)> {
    const T: usize = 1024;
    let (y0, rows, x0, gw, _) = *u;
    let mut keys = vec![0u64; T];
    let mut count = vec![0u64; T];
    let key = |i: usize| slots.iter().fold(0u64, |k, p| (k << 16) | p[i] as u64);
    let hash = |k: u64| (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 54) as usize;
    // The slot of `k`, inserting it; `None` past 256 tuples.
    let mut n = 0usize;
    let mut find = |k: u64, count: &[u64], keys: &mut [u64]| -> Option<usize> {
        let mut h = hash(k);
        while count[h] > 0 && keys[h] != k {
            h = (h + 1) & (T - 1);
        }
        if count[h] == 0 {
            n += 1;
            keys[h] = k;
        }
        (n <= 256).then_some(h)
    };
    // No sentinel key: four 16-bit slots of 0xFFFF pack to u64::MAX.
    let mut last: Option<(u64, usize)> = None;
    for y in y0..y0 + rows {
        for i in y * w + x0..y * w + x0 + gw {
            let k = key(i);
            let h = match last {
                Some((lk, lh)) if lk == k => lh,
                _ => find(k, &count, &mut keys)?,
            };
            count[h] += 1;
            last = Some((k, h));
        }
    }
    let mut order: Vec<usize> = (0..T).filter(|&h| count[h] > 0).collect();
    order.sort_unstable_by_key(|&h| (core::cmp::Reverse(count[h]), keys[h]));
    let mut idx_of = vec![0u8; T];
    for (i, &h) in order.iter().enumerate() {
        idx_of[h] = i as u8;
    }
    let mut idx = Vec::with_capacity(rows * gw);
    let mut last: Option<(u64, u8)> = None;
    for y in y0..y0 + rows {
        for i in y * w + x0..y * w + x0 + gw {
            let k = key(i);
            let v = match last {
                Some((lk, lv)) if lk == k => lv,
                _ => {
                    let mut h = hash(k);
                    while keys[h] != k {
                        h = (h + 1) & (T - 1);
                    }
                    idx_of[h]
                }
            };
            idx.push(v);
            last = Some((k, v));
        }
    }
    Some((order.iter().map(|&h| keys[h]).collect(), idx))
}

/// A unit's own entries when it uses at most two of more than two shared
/// `entries` (`idx` indexes them, rewritten to index the result), most
/// frequent first, ties in ascending tuple order; `None` keeps the shared
/// ones.
#[cfg(feature = "encode")]
pub(crate) fn local_entries(entries: &[u64], idx: &mut [u8]) -> Option<Vec<u64>> {
    if entries.len() <= 2 {
        return None;
    }
    let mut count = [0u64; 256];
    for &i in idx.iter() {
        count[i as usize] += 1;
    }
    let mut used: Vec<usize> = (0..256).filter(|&i| count[i] > 0).collect();
    if used.len() > 2 {
        return None;
    }
    used.sort_unstable_by_key(|&i| (core::cmp::Reverse(count[i]), entries[i]));
    let mut map = [0u8; 256];
    for (j, &i) in used.iter().enumerate() {
        map[i] = j as u8;
    }
    for i in idx.iter_mut() {
        *i = map[*i as usize];
    }
    Some(used.iter().map(|&i| entries[i]).collect())
}

/// Phase 0 context: bit 9 = [W is 0], bits 0-8 = [t_k == W].
#[cfg(feature = "encode")]
fn ctx0(t: &[u32; 10]) -> usize {
    let w = t[9];
    let mut c = ((w == 0) as usize) << 9;
    for (k, &x) in t[..9].iter().enumerate() {
        c |= ((x == w) as usize) << k;
    }
    c
}

/// The coding of one pixel of index `v` in a stripe of `n` entries:
/// `(phase, context, symbol)` per symbol (phase 6 = escape, its symbol the
/// index).
#[cfg(feature = "encode")]
fn symbols(t: &[u32; 10], n: usize, v: u32, mut f: impl FnMut(u8, usize, u32)) {
    let w = t[9];
    let miss = v != w;
    f(0, ctx0(t), miss as u32);
    if !miss || n <= 2 {
        return;
    }
    for (r, &ci) in CAND.iter().enumerate() {
        let c = t[ci];
        if c == w || CAND[..r].iter().any(|&p| t[p] == c) {
            continue;
        }
        let cnt = t.iter().filter(|&&x| x == c).count().min(7);
        f(1 + r as u8, r * 8 + cnt, (v != c) as u32);
        if v == c {
            return;
        }
    }
    f(6, 0, v);
}

/// Rows of a unit equal to the row above (the first row: all index 0).
#[cfg(feature = "encode")]
fn typical_rows(idx: &[u8], gw: usize, rows: usize) -> Vec<bool> {
    (0..rows)
        .map(|r| {
            let row = &idx[r * gw..(r + 1) * gw];
            if r == 0 {
                row.iter().all(|&v| v == 0)
            } else {
                row == &idx[(r - 1) * gw..r * gw]
            }
        })
        .collect()
}

/// A unit's indices (`gw` per row) with two rows of 0 above and two
/// columns of 0 on each side, `gw + 4` per row.
#[cfg(feature = "encode")]
fn padded(idx: &[u8], gw: usize, rows: usize) -> Vec<u8> {
    let pw = gw + 4;
    let mut v = vec![0u8; (rows + 2) * pw];
    for (r, row) in idx.chunks_exact(gw).take(rows).enumerate() {
        v[(r + 2) * pw + 2..(r + 2) * pw + 2 + gw].copy_from_slice(row);
    }
    v
}

/// Every symbol of lane `l` of a unit in decode order, skipping typical
/// rows: `f(step, phase, context, symbol)`. `pad` holds the unit's indices,
/// [`padded`].
#[cfg(feature = "encode")]
fn walk_lane(
    pad: &[u8],
    u: &Unit,
    n: usize,
    typical: &[bool],
    l: usize,
    mut f: impl FnMut(u32, u8, usize, u32),
) {
    let (_, rows, _, gw, lw) = *u;
    let pw = gw + 4;
    let width = crate::v1::lane_widths(gw, lw)[l] as usize;
    let lane0 = l * lw;
    for r in (0..rows).filter(|&r| !typical[r]) {
        let a2 = &pad[r * pw..(r + 1) * pw];
        let a1 = &pad[(r + 1) * pw..(r + 2) * pw];
        let a0 = &pad[(r + 2) * pw..(r + 3) * pw];
        for x in 0..width {
            let c = lane0 + x + 2;
            // Current-row pixels left of the lane read the row above.
            let (ww, w) = match x {
                0 => (a1[c - 2], a1[c - 1]),
                1 => (a1[c - 2], a0[c - 1]),
                _ => (a0[c - 2], a0[c - 1]),
            };
            let t = [
                a2[c - 1],
                a2[c],
                a2[c + 1],
                a1[c - 2],
                a1[c - 1],
                a1[c],
                a1[c + 1],
                a1[c + 2],
                ww,
                w,
            ]
            .map(u32::from);
            let step = (r * lw + x) as u32;
            symbols(&t, n, a0[c] as u32, |ph, ctx, s| f(step, ph, ctx, s));
        }
    }
}

/// The coded value of the probability of a 0 at `b` bits (SPEC 2.14) from a
/// context's counts: the extremes only for one-sided counts.
#[cfg(feature = "encode")]
fn quant(c0: u64, c1: u64, b: u32) -> u32 {
    let t = c0 + c1;
    // At 11 bits the value is the probability (0..=M); below, v * 2^(10 - b)
    // with v < 2^b.
    let (scale, top) = if b == 11 {
        (LOG_M, M as u64)
    } else {
        (b, (1u64 << b) - 1)
    };
    match (c0, c1) {
        (0, 0) => 0,
        (_, 0) => top as u32,
        (0, _) => 0,
        _ => (((c0 << scale) + t / 2) / t).clamp(1, top - (b == 11) as u64) as u32,
    }
}

/// The probability (in 1/M) of a coded value at `b` bits.
#[cfg(feature = "encode")]
fn expand(v: u32, b: u32) -> u32 {
    if b == 11 { v } else { v << (10 - b) }
}

/// Estimated bits of the record's probabilities and the binary symbols
/// they code, at `b` bits.
#[cfg(feature = "encode")]
fn table_cost(counts: &[[u64; 2]], b: u32) -> f64 {
    let lg = |p: u32| crate::mathx::log2(p as f64);
    counts
        .iter()
        .map(|&[c0, c1]| {
            if c0 + c1 == 0 {
                return 1.0;
            }
            let p = expand(quant(c0, c1, b), b);
            let mut bits = 1.0 + b as f64;
            if c0 > 0 {
                bits += c0 as f64 * (LOG_M as f64 - lg(p));
            }
            if c1 > 0 {
                bits += c1 as f64 * (LOG_M as f64 - lg(M - p));
            }
            bits
        })
        .sum()
}

/// An encoder unit's input: its index plane (`gw` per row, most frequent
/// entry 0) and its entry count.
#[cfg(feature = "encode")]
pub(crate) struct UnitIndices<'a> {
    pub(crate) unit: Unit,
    pub(crate) idx: &'a [u8],
    pub(crate) n: usize,
}

/// Shared entries for record 35: tuples (16 bits per slot, slot 0 highest)
/// and the slots' depths.
#[cfg(feature = "encode")]
pub(crate) type Shared<'a> = (&'a [u64], &'a [u32]);

/// The template coding of `units`: head record 35 (with `shared` entries)
/// and, per unit, the bytes after the stripe's entries (the typical-row
/// bitmap and the stream).
#[cfg(feature = "encode")]
pub(crate) fn encode(
    units: &[UnitIndices<'_>],
    shared: Option<Shared<'_>>,
    threads: usize,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    let typical: Vec<Vec<bool>> = units
        .iter()
        .map(|u| typical_rows(u.idx, u.unit.3, u.unit.1))
        .collect();
    let items: Vec<usize> = (0..units.len()).collect();
    let hists = crate::par::par_map(items.clone(), threads, |_, i| {
        let u = &units[i];
        let mut a = vec![[0u64; 2]; 1024];
        let mut b = vec![[0u64; 2]; NBCTX];
        let mut e = vec![0u64; 256];
        let pad = padded(u.idx, u.unit.3, u.unit.1);
        for l in 0..NL {
            walk_lane(
                &pad,
                &u.unit,
                u.n,
                &typical[i],
                l,
                |_, ph, ctx, s| match ph {
                    0 => a[ctx][s as usize] += 1,
                    1..=5 => b[ctx][s as usize] += 1,
                    _ => e[s as usize] += 1,
                },
            );
        }
        (a, b, e)
    });
    let mut a = vec![[0u64; 2]; 1024];
    let mut b = vec![[0u64; 2]; NBCTX];
    let mut e = vec![0u64; 256];
    for (ha, hb, he) in &hists {
        for (x, y) in a.iter_mut().zip(ha) {
            x[0] += y[0];
            x[1] += y[1];
        }
        for (x, y) in b.iter_mut().zip(hb) {
            x[0] += y[0];
            x[1] += y[1];
        }
        for (x, y) in e.iter_mut().zip(he) {
            *x += y;
        }
    }
    // The precision with the fewest record and symbol bits.
    let prec = (4..=11u32)
        .map(|p| (table_cost(&a, p) + table_cost(&b, p), p))
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .unwrap()
        .1;
    let mut bw = BitWriter::new();
    bw.put(prec - 4, 3);
    let mut put = |c: &[[u64; 2]]| -> Vec<u32> {
        c.iter()
            .map(|&[c0, c1]| {
                let v = quant(c0, c1, prec);
                bw.put((c0 + c1 > 0) as u32, 1);
                if c0 + c1 > 0 {
                    bw.put(v, prec);
                    expand(v, prec)
                } else {
                    M / 2
                }
            })
            .collect()
    };
    let qa = put(&a);
    let qb = put(&b);
    let esc = e.iter().any(|&c| c > 0);
    bw.put(esc as u32, 1);
    let freq = if esc {
        let (wt, f, prec) = rans::choose_table(&e);
        rans::write_weights(&mut bw, &wt, prec);
        f
    } else {
        let mut f = vec![0u32; 256];
        f[0] = M;
        f
    };
    bw.put(shared.is_some() as u32, 1);
    if let Some((entries, bits)) = shared {
        bw.put(entries.len() as u32 - 1, 8);
        let ns = bits.len();
        for &k in entries {
            for (s, &d) in bits.iter().enumerate() {
                bw.put((k >> (16 * (ns - 1 - s))) as u32 & 0xFFFF, d);
            }
        }
    }
    let mut cum = vec![0u32; 257];
    for s in 0..256 {
        cum[s + 1] = cum[s] + freq[s];
    }
    let streams = crate::par::par_map(items, threads, |_, i| {
        encode_unit(&units[i], &typical[i], &qa, &qb, &freq, &cum)
    });
    (bw.finish(), streams)
}

/// One unit: the typical-row bitmap, then the stream. Lanes are coded one
/// at a time; symbols and words pack into u64s (symbol: step, phase,
/// frequency, cumulative frequency; word: step, phase, lane, word), so the
/// words sort into the decoder's order (step, phase, lane).
#[cfg(feature = "encode")]
fn encode_unit(
    u: &UnitIndices<'_>,
    typical: &[bool],
    a: &[u32],
    b: &[u32],
    freq: &[u32],
    cum: &[u32],
) -> Vec<u8> {
    let (_, rows, _, gw, lw) = u.unit;
    let active = crate::v1::active_lanes(&crate::v1::lane_widths(gw, lw));
    let mut states = [RANS_L; NL];
    let mut syms: Vec<u64> = Vec::new();
    let mut words: Vec<u64> = Vec::new();
    let pad = padded(u.idx, gw, rows);
    for (l, state) in states.iter_mut().enumerate().take(active) {
        syms.clear();
        walk_lane(&pad, &u.unit, u.n, typical, l, |step, ph, ctx, s| {
            let (f, c) = match ph {
                0..=5 => {
                    let p = if ph == 0 { a[ctx] } else { b[ctx] };
                    if s == 1 { (M - p, p) } else { (p, 0) }
                }
                _ => (freq[s as usize], cum[s as usize]),
            };
            syms.push((step as u64) << 24 | (ph as u64) << 21 | (f as u64) << 10 | c as u64);
        });
        let mut x = RANS_L;
        for &s in syms.iter().rev() {
            let (f, c) = ((s >> 10 & 0x7FF) as u32, (s & 0x3FF) as u32);
            debug_assert!(f > 0);
            if x as u64 >= (f as u64) << 22 {
                words.push((s >> 21) << 20 | (l as u64) << 16 | (x & 0xFFFF) as u64);
                x >>= 16;
            }
            x = ((x / f) << LOG_M) + x % f + c;
        }
        *state = x;
    }
    let mut out = Vec::with_capacity(rows.div_ceil(8) + 4 * active + 2 * words.len());
    let mut bits = vec![0u8; rows.div_ceil(8)];
    for (r, &on) in typical.iter().enumerate() {
        bits[r / 8] |= (on as u8) << (r % 8);
    }
    out.extend_from_slice(&bits);
    for &x in &states[..active] {
        out.extend_from_slice(&x.to_le_bytes());
    }
    words.sort_unstable();
    for w in words {
        out.extend_from_slice(&(w as u16).to_le_bytes());
    }
    out
}

// ---- decoder ------------------------------------------------------------------

/// The typical-row bitmap of a unit of `rows` rows and the rest of its
/// bytes (SPEC 2.14: bits past the last row must be 0).
pub(crate) fn split_rows(bytes: &[u8], rows: usize) -> Result<(Vec<bool>, &[u8]), Error> {
    let nb = rows.div_ceil(8);
    if bytes.len() < nb {
        return Err(Error::Corrupt("short typical-row bitmap"));
    }
    let (bits, rest) = bytes.split_at(nb);
    if !rows.is_multiple_of(8) && bits[nb - 1] >> (rows % 8) != 0 {
        return Err(Error::Corrupt("typical-row bitmap padding"));
    }
    Ok((
        (0..rows).map(|r| bits[r / 8] >> (r % 8) & 1 == 1).collect(),
        rest,
    ))
}

/// A template palette unit's decoder rows: three step-major rows with two
/// halo steps before and three after, the two history rows named by index
/// (they alias after a typical row).
pub(crate) struct Rows {
    b: [Vec<u32>; 3],
    /// Buffers of the rows two and one above.
    p2: usize,
    p1: usize,
    /// The unit's typical rows and the next row to decode.
    typical: Vec<bool>,
    row: usize,
    /// Entry count.
    n: usize,
    /// Halo steps: `(destination, source)`, `u32::MAX` for outside (0).
    plan: Vec<(u32, u32)>,
    /// Output pixel bytes per index (256 entries; built on the first row).
    pub(crate) tbl: Vec<u8>,
}

impl Rows {
    pub(crate) fn new(lw: usize, gw: usize, n: usize, typical: Vec<bool>) -> Rows {
        let len = (lw + 5) * NL;
        let src = |col: isize| -> u32 {
            if col < 0 || col as usize >= gw {
                u32::MAX
            } else {
                ((col as usize % lw + 2) * NL + col as usize / lw) as u32
            }
        };
        let mut plan = Vec::with_capacity(4 * NL);
        for l in 0..NL {
            let lane0 = (l * lw) as isize;
            for k in 0..2 {
                plan.push(((k * NL + l) as u32, src(lane0 - 2 + k as isize)));
                plan.push((
                    ((lw + 2 + k) * NL + l) as u32,
                    src(lane0 + lw as isize + k as isize),
                ));
            }
        }
        Rows {
            b: [vec![0; len], vec![0; len], vec![0; len]],
            p2: 0,
            p1: 0,
            typical,
            row: 0,
            n,
            plan,
            tbl: Vec::new(),
        }
    }

    /// Decodes the next row of the unit into `out` through `tbl`.
    pub(crate) fn decode(
        &mut self,
        t: &Table,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        out: &mut [u8],
    ) {
        self.decode_with(row, t, words, st, widths, lw, out)
    }

    /// [`Rows::decode`] with the row kernel given (tier tests).
    #[allow(clippy::too_many_arguments)]
    fn decode_with(
        &mut self,
        row: kernel::RowFn,
        t: &Table,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        out: &mut [u8],
    ) {
        let r = self.row;
        self.row += 1;
        if self.typical[r] {
            // The row above, unchanged (its halo steps included).
            self.p2 = self.p1;
            emit(&self.tbl, &self.b[self.p1][2 * NL..], widths, lw, out);
            return;
        }
        let ci = (0..3).find(|&i| i != self.p1 && i != self.p2).unwrap();
        let mut cur = core::mem::take(&mut self.b[ci]);
        let (p2, p1) = (&self.b[self.p2], &self.b[self.p1]);
        // Current-row pixels left of a lane read the row above.
        cur[..2 * NL].copy_from_slice(&p1[..2 * NL]);
        row(t, self.n, words, st, widths, lw, p2, p1, &mut cur);
        for &(d, s) in &self.plan {
            cur[d as usize] = if s == u32::MAX { 0 } else { cur[s as usize] };
        }
        emit(&self.tbl, &cur[2 * NL..], widths, lw, out);
        self.b[ci] = cur;
        self.p2 = self.p1;
        self.p1 = ci;
    }
}

/// Writes one row of step-major indices through `tbl` (256 entries of the
/// pixel's bytes), lane by lane, eight pixels per iteration.
fn emit(tbl: &[u8], idx: &[u32], widths: &[u32; NL], lw: usize, out: &mut [u8]) {
    match tbl.len() / 256 {
        1 => emit_row::<1>(tbl, idx, widths, lw, out),
        2 => emit_row::<2>(tbl, idx, widths, lw, out),
        3 => emit_row::<3>(tbl, idx, widths, lw, out),
        4 => emit_row::<4>(tbl, idx, widths, lw, out),
        6 => emit_row::<6>(tbl, idx, widths, lw, out),
        8 => emit_row::<8>(tbl, idx, widths, lw, out),
        _ => crate::v1::expand(tbl, idx, widths, lw, out),
    }
}

fn emit_row<const B: usize>(
    tbl: &[u8],
    idx: &[u32],
    widths: &[u32; NL],
    lw: usize,
    out: &mut [u8],
) {
    let tbl: &[[u8; B]; 256] = tbl.as_chunks::<B>().0.try_into().unwrap();
    let out = out.as_chunks_mut::<B>().0;
    for (l, &wl) in widths.iter().enumerate() {
        let wl = wl as usize;
        if wl == 0 {
            break;
        }
        let o = &mut out[l * lw..l * lw + wl];
        let src = &idx[l..];
        let (blocks, tail) = o.as_chunks_mut::<8>();
        for (k, ob) in blocks.iter_mut().enumerate() {
            let sb: &[u32; 7 * NL + 1] =
                src[k * 8 * NL..k * 8 * NL + 7 * NL + 1].try_into().unwrap();
            for (j, o) in ob.iter_mut().enumerate() {
                *o = tbl[sb[j * NL] as u8 as usize];
            }
        }
        let done = blocks.len() * 8;
        for (j, o) in tail.iter_mut().enumerate() {
            *o = tbl[src[(done + j) * NL] as u8 as usize];
        }
    }
}

mod kernel {
    use super::*;
    use archmage::prelude::*;

    /// Marks the branch that calls it as rarely taken.
    #[cold]
    #[inline(never)]
    fn cold() {}

    type V<T> = ::magetypes::simd::generic::u32x16<T>;

    /// Lookups of 16 lanes in a power-of-two table: one hardware gather with
    /// AVX-512, lane by lane elsewhere.
    pub(super) trait Look: ::magetypes::simd::backends::U32x16Backend {
        /// Look the next step's probabilities up at every step, speculating
        /// on the outcome, rather than only after a step where every lane
        /// repeated W. Pays where a lookup is one hardware gather (AVX-512):
        /// 1.11x on 30 images on Zen 4, against 0.88x with AVX2 there and
        /// 0.91x with NEON on the M4 Pro (benchmarks/template_palettes_2026-10-15.md).
        const EAGER: bool = false;

        #[inline(always)]
        fn look<const N: usize>(self, tbl: &[u32; N], idx: V<Self>) -> V<Self> {
            V::from_array_t(self, idx.to_array().map(|i| tbl[i as usize & (N - 1)]))
        }

        /// `bk[cnt]` per lane, `cnt` in `0..8`. `PICK` selects the
        /// single-instruction path where it exists (AVX-512); elsewhere the
        /// baseline blend chain keeps codegen identical.
        const PICK: bool = false;
        #[inline(always)]
        fn pick(self, bk: &[u32; 8], cnt: V<Self>) -> V<Self> {
            let mut pb = V::splat_t(self, bk[0]);
            for j in 1..8 {
                pb = V::blend(
                    cnt.simd_eq(V::splat_t(self, j as u32)),
                    V::splat_t(self, bk[j]),
                    pb,
                );
            }
            pb
        }
    }
    #[cfg(target_arch = "x86_64")]
    impl Look for X64V4Token {
        const EAGER: bool = true;
        const PICK: bool = true;

        #[inline(always)]
        fn look<const N: usize>(self, tbl: &[u32; N], idx: V<Self>) -> V<Self> {
            V::gather_wrapping(tbl, idx)
        }

        #[arcane(_self = X64V4Token, import_intrinsics)]
        fn pick(self, bk: &[u32; 8], cnt: V<Self>) -> V<Self> {
            let bk = _mm256_loadu_si256(bk);
            V::from_repr_t(
                self,
                _mm512_permutexvar_epi32(cnt.into_repr(), _mm512_castsi256_si512(bk)),
            )
        }
    }
    #[cfg(target_arch = "x86_64")]
    impl Look for X64V3Token {}
    #[cfg(target_arch = "aarch64")]
    impl Look for NeonToken {}
    #[cfg(target_arch = "wasm32")]
    impl Look for Wasm128Token {}
    impl Look for ScalarToken {}

    /// One row, the 16 lanes as one vector (SPEC 2.14). `p2`, `p1` and
    /// `cur` are step-major with two halo steps before and three after.
    #[magetypes(define(u32x16), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn row_impl(
        token: Token,
        t: &Table,
        n: usize,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
    ) {
        let splat = |v: u32| u32x16::splat_t(token, v);
        let ld = |b: &[u32], s: usize| u32x16::from_slice_t(token, &b[s * NL..]);
        let wv = u32x16::from_array_t(token, *widths);
        let mut xs = u32x16::from_array_t(token, st.x);
        let mut wp = st.wp;
        let (zero, one) = (splat(0), splat(1));
        if n <= 2 {
            // The value context of bilevel stripes, sliding one step at a
            // time, with the probability looked up one step ahead.
            let look = |b: u32x16| token.look(&t.pairs, b);
            let mut base = zero;
            for (k, (s, from_p2)) in [
                (1, true),
                (2, true),
                (3, true),
                (0, false),
                (1, false),
                (2, false),
                (3, false),
                (4, false),
            ]
            .into_iter()
            .enumerate()
            {
                base |= ld(if from_p2 { p2 } else { p1 }, s).shl_uniform(k as u32);
            }
            base |= ld(cur, 0).shl_const::<8>();
            let mut prev = ld(cur, 1);
            let mut pp = look(base);
            for x in 0..lw {
                let p = u32x16::blend(
                    prev.simd_ne(zero),
                    pp.shr_logical_const::<16>(),
                    pp & splat(0xFFFF),
                );
                base = ((base | prev.shl_const::<9>()).shr_logical_const::<1>() & splat(0x17B))
                    | ld(p2, x + 4).shl_const::<2>()
                    | ld(p1, x + 5).shl_const::<7>();
                pp = look(base);
                let slot = xs & splat(M - 1);
                let bit = slot.simd_ge(p);
                let f = u32x16::blend(bit, splat(M) - p, p);
                let nx = f * xs.shr_logical_const::<10>() + slot - (bit & p);
                let act = wv.simd_gt(splat(x as u32));
                xs = u32x16::blend(act, nx, xs);
                let need = act & xs.simd_lt(splat(RANS_L));
                if need.any_true() {
                    let mut m = need.bitmask();
                    let mut a = xs.to_array();
                    while m != 0 {
                        let i = m.trailing_zeros() as usize;
                        a[i] = (a[i] << 16) | words.word(wp) as u32;
                        wp += 1;
                        m &= m - 1;
                    }
                    xs = u32x16::from_array_t(token, a);
                }
                prev = act & (prev ^ (bit & one));
                prev.store((&mut cur[(x + 2) * NL..(x + 3) * NL]).try_into().unwrap());
            }
            st.x = xs.to_array();
            st.wp = wp;
            return;
        }
        macro_rules! look {
            ($c:expr) => {
                token.look(&t.a, $c)
            };
        }
        // Renormalizes the lanes of `on`, in lane order.
        macro_rules! renorm {
            ($on:expr) => {{
                let need = $on & xs.simd_lt(splat(RANS_L));
                if need.any_true() {
                    let mut m = need.bitmask();
                    let mut a = xs.to_array();
                    while m != 0 {
                        let i = m.trailing_zeros() as usize;
                        a[i] = (a[i] << 16) | words.word(wp) as u32;
                        wp += 1;
                        m &= m - 1;
                    }
                    xs = u32x16::from_array_t(token, a);
                }
            }};
        }
        // One masked binary phase: the lanes of `on` decode a symbol; the
        // value is the mask of the lanes that decoded a 1.
        macro_rules! phase {
            ($p:expr, $on:expr) => {{
                let (p, on) = ($p, $on);
                let slot = xs & splat(M - 1);
                let bit = slot.simd_ge(p);
                let f = u32x16::blend(bit, splat(M) - p, p);
                let nx = f * xs.shr_logical_const::<10>() + slot - (bit & p);
                xs = u32x16::blend(on, nx, xs);
                renorm!(on);
                on & bit
            }};
        }
        // Phase 0 context of step `s`: bit 9 = [W is 0], bit 8 = [WW == W],
        // bits 0-7 = [above pixel == W].
        macro_rules! ctx {
            ($s:expr, $w:expr, $ww_eq:expr) => {{
                let (s, w): (usize, u32x16) = ($s, $w);
                let mut c = (w.simd_eq(zero) & splat(512)) | ($ww_eq & splat(256));
                c |= ld(p2, s - 1).simd_eq(w) & splat(1);
                c |= ld(p2, s).simd_eq(w) & splat(2);
                c |= ld(p2, s + 1).simd_eq(w) & splat(4);
                c |= ld(p1, s - 2).simd_eq(w) & splat(8);
                c |= ld(p1, s - 1).simd_eq(w) & splat(16);
                c |= ld(p1, s).simd_eq(w) & splat(32);
                c |= ld(p1, s + 1).simd_eq(w) & splat(64);
                c | (ld(p1, s + 2).simd_eq(w) & splat(128))
            }};
        }
        let all = splat(u32::MAX);
        let mut ww = ld(cur, 0);
        let mut w = ld(cur, 1);
        let mut p = look!(ctx!(2, w, ww.simd_eq(w)));
        // After a step where every lane repeated W (with EAGER, at every
        // step), the next step's probabilities are looked up off the
        // decode's dependency chain, assuming the lanes repeat W again (and,
        // once a lane differs, assuming it took N). Otherwise steps with
        // differing lanes look up once, after decoding.
        let mut ahead = true;
        for x in 0..lw {
            let s = x + 2;
            let ahead_now = ahead || <Token as Look>::EAGER;
            let spec = if ahead_now {
                look!(ctx!(s + 1, w, all))
            } else {
                zero
            };
            let act = wv.simd_gt(splat(x as u32));
            let miss = phase!(p, act);
            if !miss.any_true() {
                (w & act).store((&mut cur[s * NL..s * NL + NL]).try_into().unwrap());
                ww = w;
                p = if ahead_now {
                    spec
                } else {
                    look!(ctx!(s + 1, w, all))
                };
                ahead = true;
                continue;
            }
            cold();
            let speculated = ahead_now;
            ahead = false;
            let mut val = w;
            let mut open = miss;
            let tp = [
                ld(p2, s - 1),
                ld(p2, s),
                ld(p2, s + 1),
                ld(p1, s - 2),
                ld(p1, s - 1),
                ld(p1, s),
                ld(p1, s + 1),
                ld(p1, s + 2),
            ];
            // Candidate rank `rk` with value `c`, tried by the open lanes
            // where it differs from W and the earlier candidates `q`.
            macro_rules! cand {
                ($rk:expr, $c:expr, [$($q:expr),*]) => {{
                    let c: u32x16 = $c;
                    let fresh = open $(& c.simd_ne($q))*;
                    if fresh.any_true() {
                        // Template pixels equal to c (each equality is all ones).
                        let eq = c.simd_eq(tp[0]) + c.simd_eq(tp[1]) + c.simd_eq(tp[2])
                            + c.simd_eq(tp[3]) + c.simd_eq(tp[4]) + c.simd_eq(tp[5])
                            + c.simd_eq(tp[6]) + c.simd_eq(tp[7]) + c.simd_eq(ww)
                            + c.simd_eq(w);
                        let cnt = (zero - eq).min(splat(7));
                        let bk: &[u32; 8] = t.b[$rk * 8..$rk * 8 + 8].try_into().unwrap();
                        let pb = if Token::PICK {
                            token.pick(bk, cnt)
                        } else {
                            let mut pb = splat(bk[0]);
                            for j in 1..8 {
                                pb = u32x16::blend(cnt.simd_eq(splat(j as u32)), splat(bk[j]), pb);
                            }
                            pb
                        };
                        let hit = fresh & phase!(pb, fresh).not();
                        val = u32x16::blend(hit, c, val);
                        open &= hit.not();
                    }
                }};
            }
            // The next step's probabilities if the lanes that differ take N
            // (the likeliest outcome), looked up while the candidates decode.
            let spec_n = if speculated {
                look!(ctx!(s + 1, tp[5], w.simd_eq(tp[5])))
            } else {
                zero
            };
            cand!(0, tp[5], [w]);
            if open.any_true() {
                cand!(1, tp[6], [w, tp[5]]);
                if open.any_true() {
                    cand!(2, tp[4], [w, tp[5], tp[6]]);
                    if open.any_true() {
                        cand!(3, ww, [w, tp[5], tp[6], tp[4]]);
                        if open.any_true() {
                            cand!(4, tp[1], [w, tp[5], tp[6], tp[4], ww]);
                        }
                    }
                }
            }
            if open.any_true() {
                // Escapes: every open lane's state update, then their
                // renormalizations in lane order.
                let e = token.look(&t.dec, xs & splat(M - 1));
                let nx = (e.shr_logical_const::<8>() & splat(0x7FF)) * xs.shr_logical_const::<10>()
                    + e.shr_logical_const::<19>();
                xs = u32x16::blend(open, nx, xs);
                renorm!(open);
                val = u32x16::blend(open, e & splat(0xFF), val);
            }
            (val & act).store((&mut cur[s * NL..s * NL + NL]).try_into().unwrap());
            p = if speculated && !(miss & val.simd_ne(tp[5])).any_true() {
                u32x16::blend(miss, spec_n, spec)
            } else {
                look!(ctx!(s + 1, val, w.simd_eq(val)))
            };
            ww = w;
            w = val;
        }
        st.x = xs.to_array();
        st.wp = wp;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn row(
        t: &Table,
        n: usize,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
    ) {
        if crate::v1::force_scalar() {
            return row_impl_scalar(ScalarToken, t, n, words, st, widths, lw, p2, p1, cur);
        }
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        if !crate::v1::force_avx2()
            && let Some(tok) = archmage::X64V4Token::summon()
        {
            return row_impl_v4(tok, t, n, words, st, widths, lw, p2, p1, cur);
        }
        incant!(
            row_impl(t, n, words, st, widths, lw, p2, p1, cur),
            [v3, neon, wasm128, scalar]
        );
    }

    pub(super) type RowFn = fn(
        &Table,
        usize,
        &Words<'_>,
        &mut LaneState,
        &[u32; NL],
        usize,
        &[u32],
        &[u32],
        &mut [u32],
    );

    /// Every kernel tier this host runs.
    #[cfg(all(test, feature = "encode"))]
    pub(super) fn tiers() -> Vec<(&'static str, RowFn)> {
        let mut v: Vec<(&'static str, RowFn)> = vec![("scalar", |a, b, c, d, e, f, g, h, i| {
            row_impl_scalar(ScalarToken, a, b, c, d, e, f, g, h, i)
        })];
        #[cfg(target_arch = "x86_64")]
        if X64V3Token::summon().is_some() {
            v.push(("v3", |a, b, c, d, e, f, g, h, i| {
                row_impl_v3(X64V3Token::summon().unwrap(), a, b, c, d, e, f, g, h, i)
            }));
        }
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        if X64V4Token::summon().is_some() {
            v.push(("v4", |a, b, c, d, e, f, g, h, i| {
                row_impl_v4(X64V4Token::summon().unwrap(), a, b, c, d, e, f, g, h, i)
            }));
        }
        #[cfg(target_arch = "aarch64")]
        if NeonToken::summon().is_some() {
            v.push(("neon", |a, b, c, d, e, f, g, h, i| {
                row_impl_neon(NeonToken::summon().unwrap(), a, b, c, d, e, f, g, h, i)
            }));
        }
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        v.push(("wasm128", |a, b, c, d, e, f, g, h, i| {
            row_impl_wasm128(Wasm128Token::summon().unwrap(), a, b, c, d, e, f, g, h, i)
        }));
        v
    }
}

use kernel::row;

#[cfg(all(test, feature = "encode"))]
mod tests;
