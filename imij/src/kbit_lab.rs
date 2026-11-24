#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]
//! Lab: a k-bit template-context coder for images with 2 to 32 colors (`lab`
//! feature only, not part of the format): the low-bit study's
//! generalization of bilevel stripes (`benchmarks/lowbit_2026-10-11`).
//!
//! Colors become indices by frequency (0 the commonest, which is also what
//! pixels outside the image and above the stripe read). Per pixel, in lane
//! order with the lossless coder's lanes, rANS states and renormalization:
//!
//! * phase 0: one binary symbol "the pixel differs from W" (W the left
//!   neighbour; the lane rule of bilevel stripes: current-row pixels left of
//!   the lane read the row above). Two colors: the context is the T10
//!   template on the values, so the mode is bilevel stripes exactly. More
//!   colors: bit 9 = [W is 0], bits 0-8 = [template pixel == W] for the other
//!   nine template pixels.
//! * phases 1-5, only while the pixel is unresolved: for each new candidate
//!   among N, NE, NW, WW, NN (in that order, skipping values already
//!   tried), "the pixel differs from it", context (candidate rank, number of
//!   template pixels equal to it, at most 7).
//! * phase 6: the index under one static table.
//!
//! Words are consumed in (step, phase, lane) order. Static tables per image
//! (two passes): 1024 + 40 binary probabilities (used bit + 11 bits each) and
//! the escape table (the lossless weight coder).

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::{BitReader, BitWriter};
use crate::rans::{self, LOG_M, LaneState, M, NL, RANS_L, Words};

const MAGIC: &[u8; 6] = b"IMIJK1";
/// The same coder with typical rows: each unit starts with a bitmap of its
/// rows (`ceil(rows / 8)` bytes, LSB first, unused bits zero); a set bit
/// means the row equals the one above (the first row: all index 0, what the
/// template reads above a unit) and codes no symbol.
const MAGIC_TP: &[u8; 6] = b"IMIJK2";
/// The rank variant: after a miss, one symbol names the first candidate (N,
/// NE, NW, WW, NN) equal to the pixel, or an escape, in a context of which
/// candidates are fresh (differ from W and from the earlier ones); without
/// and with typical rows.
const MAGIC_RANK: &[u8; 6] = b"IMIJK3";
const MAGIC_RANK_TP: &[u8; 6] = b"IMIJK4";
/// Rank symbols: the five candidates and the escape.
const RANKS: usize = 6;
/// Rank contexts: the fresh mask of the five candidates.
const RANK_CTX: usize = 32;
/// Template pixels (dy, dx); index 9 is W.
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
/// Candidates after W, as template indices: N, NE, NW, WW, NN. (The fast
/// decoder spells this order out: `fast_row`'s `cand!` calls.)
const CAND: [usize; 5] = [5, 6, 4, 8, 1];
const NBCTX: usize = 40;

/// Binary probability (of "same", bit 0) per context, the escape table.
struct Model {
    n: usize,
    a: Vec<u32>,
    b: Vec<u32>,
    /// Rank variant: per context, `freq[RANKS]`, `cum[RANKS + 1]` and the
    /// slot table (as the escape table's); empty otherwise.
    rank_freq: Vec<[u32; RANKS]>,
    rank_cum: Vec<[u32; RANKS + 1]>,
    rank_dec: Vec<u32>,
    /// Escape: frequencies, cumulative, and slot -> sym | freq << 8 | bias << 19.
    freq: Vec<u32>,
    cum: Vec<u32>,
    dec: Vec<u32>,
}

fn quant(c0: u64, c1: u64) -> u32 {
    let t = c0 + c1;
    match (c0, c1) {
        (0, 0) => M / 2,
        (_, 0) => M,
        (0, _) => 0,
        _ => (((c0 << LOG_M) + t / 2) / t).clamp(1, M as u64 - 1) as u32,
    }
}

fn escape_tables(freq: Vec<u32>) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut cum = vec![0u32; freq.len() + 1];
    for s in 0..freq.len() {
        cum[s + 1] = cum[s] + freq[s];
    }
    let mut dec = vec![0u32; M as usize];
    for s in 0..freq.len() {
        for slot in cum[s]..cum[s + 1] {
            dec[slot as usize] = s as u32 | freq[s] << 8 | (slot - cum[s]) << 19;
        }
    }
    (freq, cum, dec)
}

/// Cumulative and slot tables of the rank contexts (contexts without symbols
/// keep an all-zero table: no symbol is ever decoded in them).
#[allow(clippy::type_complexity)]
fn rank_tables(freq: &[[u32; RANKS]]) -> (Vec<[u32; RANKS + 1]>, Vec<u32>) {
    let mut cum = vec![[0u32; RANKS + 1]; freq.len()];
    let mut dec = vec![0u32; freq.len() * M as usize];
    for (i, f) in freq.iter().enumerate() {
        for s in 0..RANKS {
            cum[i][s + 1] = cum[i][s] + f[s];
            for slot in cum[i][s]..cum[i][s + 1] {
                dec[i * M as usize + slot as usize] =
                    s as u32 | f[s] << 8 | (slot - cum[i][s]) << 19;
            }
        }
    }
    (cum, dec)
}

/// Template values of a pixel (lane rule, outside 0) from a value reader.
#[inline(always)]
fn template(v: &impl Fn(isize, isize) -> u32, r: isize, c: isize, lane0: isize) -> [u32; 10] {
    core::array::from_fn(|i| {
        let (dy, dx) = T10[i];
        let (y, x) = (r + dy, c + dx);
        v(if dy == 0 && x < lane0 { y - 1 } else { y }, x)
    })
}

fn ctx_a(t: &[u32; 10], n: usize) -> usize {
    let w = t[9];
    if n == 2 {
        return (0..10).fold(0, |a, i| a | (t[i] as usize) << i);
    }
    let mut c = ((w == 0) as usize) << 9;
    for i in 0..9 {
        c |= ((t[i] == w) as usize) << i;
    }
    c
}

/// The rank context (fresh mask: candidate `r` differs from W and from the
/// earlier candidates) and the rank symbol of value `v` (the first candidate
/// equal to it, else the escape `RANKS - 1`).
fn rank_of(t: &[u32; 10], v: u32) -> (usize, u32) {
    let mut fresh = 0;
    let mut sym = (RANKS - 1) as u32;
    for (r, &ci) in CAND.iter().enumerate() {
        let c = t[ci];
        if c != t[9] && !CAND[..r].iter().any(|&p| t[p] == c) {
            fresh |= 1 << r;
        }
        if c == v && sym == (RANKS - 1) as u32 {
            sym = r as u32;
        }
    }
    (fresh, sym)
}

/// The coding of one pixel: `(phase, context, symbol)` per emitted symbol.
/// The rank variant (`rank`) codes a miss as phase 1 (rank symbol) and, for
/// an escape, phase 6 (the index).
fn symbols(t: &[u32; 10], n: usize, rank: bool, v: u32, mut f: impl FnMut(u8, usize, u32)) {
    let w = t[9];
    let miss = v != w;
    f(0, ctx_a(t, n), miss as u32);
    if !miss || n == 2 {
        return;
    }
    if rank {
        let (ctx, sym) = rank_of(t, v);
        f(1, ctx, sym);
        if sym == (RANKS - 1) as u32 {
            f(6, 0, v);
        }
        return;
    }
    let mut seen = [w; 6];
    let mut k = 1;
    for (r, &ci) in CAND.iter().enumerate() {
        let c = t[ci];
        if seen[..k].contains(&c) {
            continue;
        }
        seen[k] = c;
        k += 1;
        let cnt = t.iter().filter(|&&x| x == c).count().min(7);
        f(1 + r as u8, r * 8 + cnt, (v != c) as u32);
        if v == c {
            return;
        }
    }
    f(6, 0, v);
}

type Unit = (usize, usize); // first row, rows

/// Every symbol of a unit in lane order: `(lane, step, phase, ctx, sym)`,
/// leaving out the rows `skip` marks (typical rows).
fn walk(
    idx: &[u8],
    w: usize,
    u: Unit,
    lw: usize,
    n: usize,
    rank: bool,
    skip: &[bool],
    mut f: impl FnMut(usize, u32, u8, usize, u32),
) {
    let widths = crate::v1::lane_widths(w, lw);
    let v = |y: isize, x: isize| -> u32 {
        if y < 0 || x < 0 || x as usize >= w {
            0
        } else {
            idx[(u.0 + y as usize) * w + x as usize] as u32
        }
    };
    for l in 0..NL {
        let lane0 = (l * lw) as isize;
        for r in 0..u.1 {
            if skip.get(r).copied().unwrap_or(false) {
                continue;
            }
            for x in 0..widths[l] as usize {
                let c = lane0 + x as isize;
                let t = template(&v, r as isize, c, lane0);
                let step = (r * lw + x) as u32;
                symbols(
                    &t,
                    n,
                    rank,
                    idx[(u.0 + r) * w + c as usize] as u32,
                    |ph, ctx, s| f(l, step, ph, ctx, s),
                );
            }
        }
    }
}

struct Sym {
    key: u64,
    freq: u32,
    cum: u32,
}

/// Rows of a unit equal to the row above (the first row: all index 0).
fn repeated_rows(idx: &[u8], w: usize, u: Unit) -> Vec<bool> {
    (0..u.1)
        .map(|r| {
            let row = &idx[(u.0 + r) * w..(u.0 + r + 1) * w];
            if r == 0 {
                row.iter().all(|&v| v == 0)
            } else {
                row == &idx[(u.0 + r - 1) * w..(u.0 + r) * w]
            }
        })
        .collect()
}

/// A unit's stream; with typical rows (`tp`) the row bitmap comes first and
/// the rows it marks code nothing.
fn encode_unit(idx: &[u8], w: usize, u: Unit, lw: usize, md: &Model, tp: bool) -> Vec<u8> {
    let widths = crate::v1::lane_widths(w, lw);
    let skip = if tp {
        repeated_rows(idx, w, u)
    } else {
        Vec::new()
    };
    let rank = !md.rank_freq.is_empty();
    let mut lanes: [Vec<Sym>; NL] = Default::default();
    walk(idx, w, u, lw, md.n, rank, &skip, |l, step, ph, ctx, s| {
        let (freq, cum) = match ph {
            1 if rank => (md.rank_freq[ctx][s as usize], md.rank_cum[ctx][s as usize]),
            0..=5 => {
                let p = if ph == 0 { md.a[ctx] } else { md.b[ctx] };
                if s == 1 { (M - p, p) } else { (p, 0) }
            }
            _ => (md.freq[s as usize], md.cum[s as usize]),
        };
        lanes[l].push(Sym {
            key: (step as u64) << 8 | (ph as u64) << 4 | l as u64,
            freq,
            cum,
        });
    });
    let mut states = [RANS_L; NL];
    let mut words: Vec<(u64, u16)> = Vec::new();
    for (l, syms) in lanes.iter().enumerate() {
        let mut x = RANS_L;
        for s in syms.iter().rev() {
            debug_assert!(s.freq > 0);
            if x as u64 >= (s.freq as u64) << 22 {
                words.push((s.key, x as u16));
                x >>= 16;
            }
            x = ((x / s.freq) << LOG_M) + x % s.freq + s.cum;
        }
        states[l] = x;
    }
    let active = crate::v1::active_lanes(&widths);
    let mut out = Vec::with_capacity(u.1.div_ceil(8) + 4 * active + 2 * words.len());
    if tp {
        let mut bits = vec![0u8; u.1.div_ceil(8)];
        for (r, &on) in skip.iter().enumerate() {
            bits[r / 8] |= (on as u8) << (r % 8);
        }
        out.extend_from_slice(&bits);
    }
    for &x in &states[..active] {
        out.extend_from_slice(&x.to_le_bytes());
    }
    words.sort_unstable_by_key(|&(k, _)| k);
    for (_, wd) in words {
        out.extend_from_slice(&wd.to_le_bytes());
    }
    out
}

fn geometry(w: usize, h: usize) -> (usize, usize, Vec<Unit>) {
    let hs = crate::v1::default_stripe_rows(w as u32) as usize;
    let lw = w.div_ceil(NL);
    let units = (0..h.div_ceil(hs))
        .map(|s| (s * hs, hs.min(h - s * hs)))
        .collect();
    (hs, lw, units)
}

/// Event counts of the coding of an image (a measurement aid: where the
/// vector decoder's work goes). A *step* is one column position of a row of
/// a unit: the 16 lanes decode one pixel each in lockstep.
#[derive(Clone, Copy, Debug, Default)]
pub struct EventStats {
    pub colors: usize,
    pub pixels: u64,
    /// Steps with at least one active lane.
    pub steps: u64,
    /// Pixels whose phase-0 symbol is "differs from W".
    pub miss_px: u64,
    /// Steps where some lane differs from W (the vector decoder leaves its
    /// one-symbol path).
    pub miss_steps: u64,
    /// Candidate phases the vector decoder runs: per step, the ranks some
    /// lane decodes a symbol for.
    pub cand_phases: u64,
    /// Escape symbols, and steps with at least one.
    pub escape_px: u64,
    pub escape_steps: u64,
    /// All binary symbols (phases 0-5).
    pub binary_events: u64,
    /// Rows of a unit equal to the row above (the first row of a unit
    /// compares with value 0, as its template does).
    pub repeated_rows: u64,
    pub rows: u64,
}

/// [`EventStats`] of the coding [`encode`] would write; `None` where it
/// would not code the image.
pub fn event_stats(pixels: &[u8], w: usize, h: usize, ch: usize) -> Option<EventStats> {
    let (n, _, idx) = ranked(pixels, ch)?;
    let (_, lw, units) = geometry(w, h);
    let mut st = EventStats {
        colors: n,
        pixels: (w * h) as u64,
        ..Default::default()
    };
    for u in units {
        let mut mask = vec![0u8; u.1 * lw];
        walk(&idx, w, u, lw, n, false, &[], |_, step, ph, _, s| {
            let m = &mut mask[step as usize];
            *m |= 1 << ph;
            match ph {
                0 => st.miss_px += s as u64,
                6 => st.escape_px += 1,
                _ => {}
            }
            if ph < 6 {
                st.binary_events += 1;
            }
        });
        for &m in &mask {
            st.steps += (m != 0) as u64;
            // Two colors: a miss is resolved by phase 0 itself.
            st.miss_steps += (m & !1 != 0) as u64;
            st.cand_phases += (m & 0b11_1110).count_ones() as u64;
            st.escape_steps += (m & 0x40 != 0) as u64;
        }
        for r in 0..u.1 {
            let row = &idx[(u.0 + r) * w..(u.0 + r + 1) * w];
            st.rows += 1;
            st.repeated_rows += if r == 0 {
                row.iter().all(|&v| v == 0)
            } else {
                row == &idx[(u.0 + r - 1) * w..(u.0 + r) * w]
            } as u64;
        }
    }
    Some(st)
}

/// Colors by frequency (commonest first, ties by value) and the index plane;
/// `None` with more than 32 colors or fewer than two.
#[allow(clippy::type_complexity)]
fn ranked(pixels: &[u8], ch: usize) -> Option<(usize, Vec<(u64, u32)>, Vec<u8>)> {
    let mut colors: Vec<(u64, u32)> = Vec::new();
    let key = |p: &[u8]| p.iter().fold(0u32, |a, &b| a << 8 | b as u32);
    let mut counts = alloc::collections::BTreeMap::<u32, u64>::new();
    for p in pixels.chunks_exact(ch) {
        *counts.entry(key(p)).or_default() += 1;
        if counts.len() > 32 {
            return None;
        }
    }
    if counts.len() < 2 {
        return None;
    }
    for (&k, &c) in &counts {
        colors.push((c, k));
    }
    colors.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let n = colors.len();
    let rank: alloc::collections::BTreeMap<u32, u8> = colors
        .iter()
        .enumerate()
        .map(|(i, &(_, k))| (k, i as u8))
        .collect();
    let idx: Vec<u8> = pixels.chunks_exact(ch).map(|p| rank[&key(p)]).collect();
    Some((n, colors, idx))
}

/// Encodes `pixels` (`w * h` pixels of `ch` bytes) with at most 32 colors;
/// `None` when it has more (or fewer than two).
pub fn encode(pixels: &[u8], w: usize, h: usize, ch: usize, threads: usize) -> Option<Vec<u8>> {
    encode_opts(pixels, w, h, ch, threads, false, false)
}

/// [`encode`] with the rank variant (`IMIJK3`, or `IMIJK4` with typical
/// rows): after a miss, one rank symbol instead of the candidate cascade.
pub fn encode_rank(
    pixels: &[u8],
    w: usize,
    h: usize,
    ch: usize,
    threads: usize,
    typical_rows: bool,
) -> Option<Vec<u8>> {
    encode_opts(pixels, w, h, ch, threads, typical_rows, true)
}

/// [`encode`] with typical rows (`IMIJK2`): rows equal to the row above code
/// no symbols, and the probabilities count only the coded rows.
pub fn encode_typical_rows(
    pixels: &[u8],
    w: usize,
    h: usize,
    ch: usize,
    threads: usize,
) -> Option<Vec<u8>> {
    encode_opts(pixels, w, h, ch, threads, true, false)
}

fn encode_opts(
    pixels: &[u8],
    w: usize,
    h: usize,
    ch: usize,
    threads: usize,
    tp: bool,
    rank: bool,
) -> Option<Vec<u8>> {
    let (n, colors, idx) = ranked(pixels, ch)?;
    let (hs, lw, units) = geometry(w, h);
    // Pass 1: counts.
    let hists = crate::par::par_map(units.clone(), threads, |_, u| {
        let mut a = vec![[0u64; 2]; 1024];
        let mut b = vec![[0u64; 2]; NBCTX];
        let mut k = vec![[0u64; RANKS]; RANK_CTX];
        let mut e = vec![0u64; n];
        let skip = if tp {
            repeated_rows(&idx, w, u)
        } else {
            Vec::new()
        };
        walk(
            &idx,
            w,
            u,
            lw,
            n,
            rank,
            &skip,
            |_, _, ph, ctx, s| match ph {
                0 => a[ctx][s as usize] += 1,
                1 if rank => k[ctx][s as usize] += 1,
                1..=5 => b[ctx][s as usize] += 1,
                _ => e[s as usize] += 1,
            },
        );
        (a, b, e, k)
    });
    let mut a = vec![[0u64; 2]; 1024];
    let mut b = vec![[0u64; 2]; NBCTX];
    let mut k = vec![[0u64; RANKS]; RANK_CTX];
    let mut e = vec![0u64; n];
    for (ha, hb, he, hk) in &hists {
        for i in 0..RANK_CTX {
            for j in 0..RANKS {
                k[i][j] += hk[i][j];
            }
        }
        for i in 0..1024 {
            a[i][0] += ha[i][0];
            a[i][1] += ha[i][1];
        }
        for i in 0..NBCTX {
            b[i][0] += hb[i][0];
            b[i][1] += hb[i][1];
        }
        for i in 0..n {
            e[i] += he[i];
        }
    }
    let mut bw = BitWriter::new();
    let mut put = |c: &[[u64; 2]]| -> Vec<u32> {
        c.iter()
            .map(|&[c0, c1]| {
                let q = quant(c0, c1);
                bw.put((c0 + c1 > 0) as u32, 1);
                if c0 + c1 > 0 {
                    bw.put(q, 11);
                }
                q
            })
            .collect()
    };
    let qa = put(&a);
    let (qb, rank_freq) = if rank {
        // Per rank context: a used bit and its weights.
        let mut rf = vec![[0u32; RANKS]; RANK_CTX];
        for (i, c) in k.iter().enumerate() {
            let used = c.iter().any(|&v| v > 0);
            bw.put(used as u32, 1);
            if used {
                let (wt, f, prec) = rans::choose_table(c);
                rans::write_weights(&mut bw, &wt, prec);
                rf[i].copy_from_slice(&f);
            }
        }
        (vec![M / 2; NBCTX], rf)
    } else {
        (put(&b), Vec::new())
    };
    let esc = e.iter().any(|&c| c > 0);
    bw.put(esc as u32, 1);
    let freq = if esc {
        let (wt, f, prec) = rans::choose_table(&e);
        rans::write_weights(&mut bw, &wt, prec);
        f
    } else {
        let mut f = vec![0u32; n];
        f[0] = M;
        f
    };
    let (freq, cum, dec) = escape_tables(freq);
    let (rank_cum, rank_dec) = rank_tables(&rank_freq);
    let md = Model {
        n,
        a: qa,
        b: qb,
        rank_freq,
        rank_cum,
        rank_dec,
        freq,
        cum,
        dec,
    };
    let streams = crate::par::par_map(units, threads, |_, u| encode_unit(&idx, w, u, lw, &md, tp));
    let mut out = match (rank, tp) {
        (false, false) => MAGIC,
        (false, true) => MAGIC_TP,
        (true, false) => MAGIC_RANK,
        (true, true) => MAGIC_RANK_TP,
    }
    .to_vec();
    out.push(n as u8);
    out.push(ch as u8);
    crate::put_varint(&mut out, w as u64);
    crate::put_varint(&mut out, h as u64);
    crate::put_varint(&mut out, hs as u64);
    for &(_, k) in &colors {
        for b in (0..ch).rev() {
            out.push((k >> (8 * b)) as u8);
        }
    }
    let table = bw.finish();
    crate::put_varint(&mut out, table.len() as u64);
    out.extend_from_slice(&table);
    for s in &streams {
        crate::put_varint(&mut out, s.len() as u64);
        out.extend_from_slice(s);
    }
    Some(out)
}

struct Head<'a> {
    n: usize,
    ch: usize,
    w: usize,
    h: usize,
    hs: usize,
    palette: &'a [u8],
    md: Model,
    units: Vec<&'a [u8]>,
    table_bytes: usize,
    /// Typical rows: each unit starts with its row bitmap.
    tp: bool,
}

fn parse(file: &[u8]) -> Result<Head<'_>, Error> {
    let mut r = crate::ByteReader::new(file);
    let magic = r.bytes(6)?;
    if ![MAGIC, MAGIC_TP, MAGIC_RANK, MAGIC_RANK_TP].contains(&magic.try_into().unwrap()) {
        return Err(Error::Corrupt("not a k-bit lab file"));
    }
    let tp = magic == MAGIC_TP || magic == MAGIC_RANK_TP;
    let rank = magic == MAGIC_RANK || magic == MAGIC_RANK_TP;
    let n = r.u8()? as usize;
    let ch = r.u8()? as usize;
    let (w, h, hs) = (r.var_usize()?, r.var_usize()?, r.var_usize()?);
    if !(2..=32).contains(&n) || !(1..=4).contains(&ch) || w == 0 || h == 0 || hs == 0 {
        return Err(Error::Corrupt("bad k-bit lab head"));
    }
    let palette = r.bytes(n * ch)?;
    let tl = r.var_usize()?;
    let tb = r.bytes(tl)?;
    let mut br = BitReader::new(tb);
    let mut get = |k: usize| -> Result<Vec<u32>, Error> {
        (0..k)
            .map(|_| {
                if br.read_refill(1) == 1 {
                    let q = br.read_refill(11);
                    if q > M {
                        return Err(Error::Corrupt("probability above 1"));
                    }
                    Ok(q)
                } else {
                    Ok(M / 2)
                }
            })
            .collect()
    };
    let a = get(1024)?;
    let b = if rank { None } else { Some(get(NBCTX)?) };
    let (b, rank_freq) = if let Some(b) = b {
        (b, Vec::new())
    } else {
        let mut rf = vec![[0u32; RANKS]; RANK_CTX];
        for f in rf.iter_mut() {
            if br.read_refill(1) == 1 {
                let (wt, _) = rans::read_weights(&mut br, RANKS)?;
                let q = rans::weights_to_freq(&wt).ok_or(Error::Corrupt("empty rank table"))?;
                f.copy_from_slice(&q);
            }
        }
        (vec![M / 2; NBCTX], rf)
    };
    let freq = if br.read_refill(1) == 1 {
        let (wt, _) = rans::read_weights(&mut br, n)?;
        rans::weights_to_freq(&wt).ok_or(Error::Corrupt("empty escape table"))?
    } else {
        let mut f = vec![0u32; n];
        f[0] = M;
        f
    };
    if br.overrun() {
        return Err(Error::Corrupt("k-bit table overruns"));
    }
    let (freq, cum, dec) = escape_tables(freq);
    let (rank_cum, rank_dec) = rank_tables(&rank_freq);
    let mut units = Vec::new();
    for _ in 0..h.div_ceil(hs) {
        let l = r.var_usize()?;
        units.push(r.bytes(l)?);
    }
    Ok(Head {
        n,
        ch,
        w,
        h,
        hs,
        palette,
        md: Model {
            n,
            a,
            b,
            rank_freq,
            rank_cum,
            rank_dec,
            freq,
            cum,
            dec,
        },
        units,
        table_bytes: tl,
        tp,
    })
}

/// `(colors, table bytes, palette bytes, stream bytes)`.
pub fn size_info(file: &[u8]) -> Result<(usize, usize, usize, usize), Error> {
    let hd = parse(file)?;
    Ok((
        hd.n,
        hd.table_bytes,
        hd.palette.len(),
        hd.units.iter().map(|u| u.len()).sum(),
    ))
}

// ---- decoders -----------------------------------------------------------------

#[inline(always)]
fn binary(x: &mut u32, p: u32, wp: &mut usize, words: &Words<'_>) -> u32 {
    let slot = *x & (M - 1);
    let bit = slot >= p;
    let (f, b) = if bit { (M - p, slot - p) } else { (p, slot) };
    *x = f * (*x >> LOG_M) + b;
    rans::renorm_one(x, wp, words);
    bit as u32
}

#[inline(always)]
fn escape(x: &mut u32, md: &Model, wp: &mut usize, words: &Words<'_>) -> u32 {
    let e = md.dec[(*x & (M - 1)) as usize];
    *x = ((e >> 8) & 0x7FF) * (*x >> LOG_M) + (e >> 19);
    rans::renorm_one(x, wp, words);
    e & 0xFF
}

/// Reference decoder of one unit (natural order indices), phases in order.
fn decode_unit_ref(
    md: &Model,
    bytes: &[u8],
    w: usize,
    rows: usize,
    lw: usize,
    typical: &[bool],
    out: &mut [u8],
) -> Result<(), Error> {
    let widths = crate::v1::lane_widths(w, lw);
    let (words, x0) = Words::from_bytes(bytes, crate::v1::active_lanes(&widths))?;
    let mut x = x0;
    let mut wp = 0usize;
    let n = md.n;
    for r in 0..rows {
        if typical[r] {
            // The row above (index 0 above the unit), no symbols.
            if r == 0 {
                out[..w].fill(0);
            } else {
                out.copy_within((r - 1) * w..r * w, r * w);
            }
            continue;
        }
        for s in 0..lw {
            // Phase by phase across lanes: phase 0 for every lane, then the
            // candidate phases and the escape for the lanes still open.
            let mut t = [[0u32; 10]; NL];
            let mut open = [false; NL];
            let mut val = [0u32; NL];
            for l in 0..NL {
                if s >= widths[l] as usize {
                    continue;
                }
                let lane0 = (l * lw) as isize;
                let v = |y: isize, xx: isize| -> u32 {
                    if y < 0 || xx < 0 || xx as usize >= w {
                        0
                    } else {
                        out[y as usize * w + xx as usize] as u32
                    }
                };
                t[l] = template(&v, r as isize, lane0 + s as isize, lane0);
                let miss = binary(&mut x[l], md.a[ctx_a(&t[l], n)], &mut wp, &words);
                val[l] = if miss == 0 {
                    t[l][9]
                } else if n == 2 {
                    t[l][9] ^ 1
                } else {
                    open[l] = true;
                    0
                };
            }
            if !md.rank_freq.is_empty() {
                // Rank variant: one rank symbol per open lane, then the
                // escapes, each phase in lane order.
                for l in 0..NL {
                    if !open[l] {
                        continue;
                    }
                    let (ctx, _) = rank_of(&t[l], u32::MAX);
                    let e = md.rank_dec[ctx * M as usize + (x[l] & (M - 1)) as usize];
                    x[l] = ((e >> 8) & 0x7FF) * (x[l] >> LOG_M) + (e >> 19);
                    rans::renorm_one(&mut x[l], &mut wp, &words);
                    let sym = (e & 0xFF) as usize;
                    if sym < RANKS - 1 {
                        val[l] = t[l][CAND[sym]];
                        open[l] = false;
                    }
                }
            }
            let mut seen = [[0u32; 6]; NL];
            let mut k = [1usize; NL];
            for l in 0..NL {
                seen[l][0] = t[l][9];
            }
            for (rk, &ci) in CAND.iter().enumerate() {
                if !md.rank_freq.is_empty() {
                    break;
                }
                for l in 0..NL {
                    if !open[l] {
                        continue;
                    }
                    let c = t[l][ci];
                    if seen[l][..k[l]].contains(&c) {
                        continue;
                    }
                    seen[l][k[l]] = c;
                    k[l] += 1;
                    let cnt = t[l].iter().filter(|&&q| q == c).count().min(7);
                    if binary(&mut x[l], md.b[rk * 8 + cnt], &mut wp, &words) == 0 {
                        val[l] = c;
                        open[l] = false;
                    }
                }
            }
            for l in 0..NL {
                if open[l] {
                    val[l] = escape(&mut x[l], md, &mut wp, &words);
                }
            }
            for l in 0..NL {
                if s < widths[l] as usize {
                    out[r * w + l * lw + s] = val[l] as u8;
                }
            }
        }
    }
    if wp > words.len {
        return Err(Error::Corrupt("lane stream overrun"));
    }
    Ok(())
}

mod vec_row {
    use super::*;
    use archmage::prelude::*;

    /// One row, the 16 lanes as one vector. Rows are step-major with two halo
    /// steps before (the columns left of each lane) and two after.
    #[magetypes(define(u32x16), v4, v3, neon, wasm128, scalar)]
    pub(super) fn row_impl(
        token: Token,
        md: &Model,
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
        let look =
            |t: &[u32], c: u32x16| u32x16::from_array_t(token, c.to_array().map(|i| t[i as usize]));
        let n = md.n;
        let wv = u32x16::from_array_t(token, *widths);
        let mut xs = u32x16::from_array_t(token, st.x);
        let mut wp = st.wp;
        let (zero, one) = (splat(0), splat(1));
        // One masked binary phase: returns the decoded bits (0/1) of `on`.
        let phase = |xs: &mut u32x16, p: u32x16, on: u32x16, wp: &mut usize| -> u32x16 {
            let slot = *xs & splat(M - 1);
            let bit = slot.simd_ge(p);
            let f = u32x16::blend(bit, splat(M) - p, p);
            let nx = f * xs.shr_logical_const::<10>() + slot - (bit & p);
            *xs = u32x16::blend(on, nx, *xs);
            let mut m = (on & xs.simd_lt(splat(RANS_L))).bitmask();
            if m != 0 {
                let mut a = xs.to_array();
                while m != 0 {
                    let i = m.trailing_zeros() as usize;
                    a[i] = (a[i] << 16) | words.word(*wp) as u32;
                    *wp += 1;
                    m &= m - 1;
                }
                *xs = u32x16::from_array_t(token, a);
            }
            on & bit & one
        };
        for x in 0..lw {
            let s = x + 2;
            let t = [
                ld(p2, s - 1),
                ld(p2, s),
                ld(p2, s + 1),
                ld(p1, s - 2),
                ld(p1, s - 1),
                ld(p1, s),
                ld(p1, s + 1),
                ld(p1, s + 2),
                ld(cur, s - 2),
                ld(cur, s - 1),
            ];
            let w = t[9];
            let ctx = if n == 2 {
                (0..10).fold(zero, |a, i| a | t[i].shl_uniform(i as u32))
            } else {
                (0..9).fold(w.simd_eq(zero) & splat(512), |a, i| {
                    a | (t[i].simd_eq(w) & splat(1 << i))
                })
            };
            let act = wv.simd_gt(splat(x as u32));
            let miss = phase(&mut xs, look(&md.a, ctx), act, &mut wp);
            let mut val = if n == 2 { w ^ miss } else { w };
            let mut open = miss.simd_ne(zero);
            if n > 2 && open.bitmask() != 0 {
                let mut seen = [w; 6];
                for (rk, &ci) in CAND.iter().enumerate() {
                    let c = t[ci];
                    let mut fresh = open;
                    for q in &seen[..=rk] {
                        fresh &= c.simd_ne(*q);
                    }
                    seen[rk + 1] = c;
                    if fresh.bitmask() == 0 {
                        continue;
                    }
                    let cnt = (0..10)
                        .fold(zero, |a, i| a + (t[i].simd_eq(c) & one))
                        .min(splat(7));
                    let ctx = splat(rk as u32 * 8) + cnt;
                    let b = phase(&mut xs, look(&md.b, ctx), fresh, &mut wp);
                    let hit = fresh & b.simd_eq(zero);
                    val = u32x16::blend(hit, c, val);
                    open &= hit.not();
                }
                let mut m = open.bitmask();
                if m != 0 {
                    let mut xa = xs.to_array();
                    let mut va = val.to_array();
                    while m != 0 {
                        let i = m.trailing_zeros() as usize;
                        va[i] = escape(&mut xa[i], md, &mut wp, words);
                        m &= m - 1;
                    }
                    xs = u32x16::from_array_t(token, xa);
                    val = u32x16::from_array_t(token, va);
                }
            }
            (val & act).store((&mut cur[s * NL..s * NL + NL]).try_into().unwrap());
        }
        st.x = xs.to_array();
        st.wp = wp;
    }

    pub(super) fn row(
        md: &Model,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
    ) {
        incant!(
            row_impl(md, words, st, widths, lw, p2, p1, cur),
            [v4, v3, neon, wasm128, scalar]
        );
    }
}

/// Fills a row's halo steps from its decoded columns: two before (the
/// previous lane's last two columns) and two after (the next lane's first).
fn halo(row: &mut [u32], w: usize, lw: usize) {
    let at = |row: &[u32], col: isize| -> u32 {
        if col < 0 || col as usize >= w {
            0
        } else {
            row[(col as usize % lw + 2) * NL + col as usize / lw]
        }
    };
    for l in 0..NL {
        let lane0 = (l * lw) as isize;
        for k in 0..2 {
            row[k * NL + l] = at(row, lane0 - 2 + k as isize);
            row[(lw + 2 + k) * NL + l] = at(row, lane0 + lw as isize + k as isize);
        }
    }
}

fn decode_unit_vec(
    md: &Model,
    bytes: &[u8],
    w: usize,
    rows: usize,
    lw: usize,
    typical: &[bool],
    out: &mut [u8],
) -> Result<(), Error> {
    let widths = crate::v1::lane_widths(w, lw);
    let (words, x) = Words::from_bytes(bytes, crate::v1::active_lanes(&widths))?;
    let mut st = LaneState {
        x,
        run: [0; NL],
        wp: 0,
    };
    let nsteps = lw + 4;
    let mut b = [
        vec![0u32; nsteps * NL],
        vec![0u32; nsteps * NL],
        vec![0u32; nsteps * NL],
    ];
    for r in 0..rows {
        let [b0, b1, b2] = &mut b;
        let (p2, p1, cur) = match r % 3 {
            0 => (&*b1, &*b2, b0),
            1 => (&*b2, &*b0, b1),
            _ => (&*b0, &*b1, b2),
        };
        if typical[r] {
            // The row above, halo steps included; no symbols.
            cur.copy_from_slice(p1);
        } else {
            // Current-row pixels left of a lane read the row above.
            cur[..2 * NL].copy_from_slice(&p1[..2 * NL]);
            vec_row::row(md, &words, &mut st, &widths, lw, p2, p1, cur);
            halo(cur, w, lw);
        }
        for l in 0..NL {
            for s in 0..widths[l] as usize {
                out[r * w + l * lw + s] = cur[(s + 2) * NL + l] as u8;
            }
        }
    }
    if st.wp > words.len {
        return Err(Error::Corrupt("lane stream overrun"));
    }
    Ok(())
}

/// The decoder's tables as fixed-size arrays (lookups need no bounds checks),
/// built once per file from [`Model`]; the coding is unchanged.
struct FastModel {
    n: usize,
    /// Prepare the next step only after a step without misses.
    adaptive: bool,
    /// Escapes one open lane at a time instead of one 16-lane lookup.
    lane_escape: bool,
    /// Two colors: `pairs[b]` holds the probability of "same as W" in
    /// context `b` (bits 0-8) with bit 9 clear in its low half and set in its
    /// high half, as bilevel stripes' table.
    pairs: [u32; 512],
    /// Phase 0, more colors.
    a: [u32; 1024],
    /// Phases 1-5: `b[rank * 8 + count]`.
    b: [u32; 64],
    /// The escape decode table (slot -> sym | freq << 8 | bias << 19).
    dec: [u32; M as usize],
    /// Rank variant: the rank slot tables, `context << 10 | slot`.
    rank_dec: Option<alloc::boxed::Box<[u32; RANK_CTX * M as usize]>>,
}

impl FastModel {
    fn new(md: &Model, adaptive: bool, lane_escape: bool) -> alloc::boxed::Box<FastModel> {
        let mut f = alloc::boxed::Box::new(FastModel {
            n: md.n,
            adaptive,
            lane_escape,
            pairs: [0; 512],
            a: [0; 1024],
            b: [0; 64],
            dec: [0; M as usize],
            rank_dec: (!md.rank_freq.is_empty()).then(|| {
                md.rank_dec
                    .clone()
                    .into_boxed_slice()
                    .try_into()
                    .expect("rank table size")
            }),
        });
        f.a.copy_from_slice(&md.a);
        f.b[..NBCTX].copy_from_slice(&md.b);
        f.pairs = core::array::from_fn(|i| md.a[i] | md.a[i | 512] << 16);
        f.dec.copy_from_slice(&md.dec);
        f
    }
}

mod fast_row {
    use super::*;
    use archmage::prelude::*;

    /// Marks the branch that calls it as rarely taken (register allocation
    /// and block placement favor the other one).
    #[cold]
    #[inline(never)]
    fn cold() {}

    /// One row, the 16 lanes as one vector, for the same symbols as
    /// [`super::vec_row`] (rows step-major, two halo steps before and three
    /// after: the third is padding for the lookahead loads).
    ///
    /// Two colors: bilevel stripes' sliding context and one-step probability
    /// lookahead, the decoded symbol being "differs from W".
    ///
    /// More colors: W and WW stay in registers. While step `x` decodes, the
    /// context and probability of step `x + 1` are prepared assuming every
    /// lane repeats W at `x` (then W and WW of `x + 1` both equal W of `x`).
    /// A step where some lane misses recomputes them from the decoded values.
    #[magetypes(define(u32x16), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
    pub(super) fn row_impl(
        token: Token,
        md: &FastModel,
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
        if md.n == 2 {
            let look = |b: u32x16| {
                u32x16::from_array_t(token, b.to_array().map(|i| md.pairs[i as usize & 511]))
            };
            // Step 0's context (bits 0-8) and W (bit 9).
            let mut base = zero;
            for (i, (s, row)) in [
                (1, 0),
                (2, 0),
                (3, 0),
                (0, 1),
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
            ]
            .into_iter()
            .enumerate()
            {
                let r = if row == 0 { p2 } else { p1 };
                base |= ld(r, s).shl_uniform(i as u32);
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
                let mut m = (act & xs.simd_lt(splat(RANS_L))).bitmask();
                if m != 0 {
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
        // Macros rather than closures: the miss path calls these several
        // times, and a closure the compiler does not inline passes its vectors
        // through memory.
        macro_rules! look {
            ($c:expr) => {
                u32x16::from_array_t(token, $c.to_array().map(|i| md.a[i as usize & 1023]))
            };
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
                let mut m = (on & xs.simd_lt(splat(RANS_L))).bitmask();
                if m != 0 {
                    let mut a = xs.to_array();
                    while m != 0 {
                        let i = m.trailing_zeros() as usize;
                        a[i] = (a[i] << 16) | words.word(wp) as u32;
                        wp += 1;
                        m &= m - 1;
                    }
                    xs = u32x16::from_array_t(token, a);
                }
                on & bit
            }};
        }
        // The eight template pixels on the rows above, at step `s`.
        let above = |s: usize| -> [u32x16; 8] {
            [
                ld(p2, s - 1),
                ld(p2, s),
                ld(p2, s + 1),
                ld(p1, s - 2),
                ld(p1, s - 1),
                ld(p1, s),
                ld(p1, s + 1),
                ld(p1, s + 2),
            ]
        };
        // Phase 0 context of step `s`: bit 9 = [W is 0], bit 8 = [WW == W],
        // bits 0-7 = [above pixel == W]; each above pixel is loaded where it
        // is used, so nothing but W, WW and the states stays live.
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
        // Whether to prepare the next step before decoding this one: always,
        // or (adaptive) only after a step where every lane repeated W, so
        // runs of missing steps do not pay for a lookup they discard.
        let mut ahead = true;
        for x in 0..lw {
            let s = x + 2;
            // Step x + 1 if every lane repeats W here.
            let spec = if ahead {
                look!(ctx!(s + 1, w, all))
            } else {
                zero
            };
            let act = wv.simd_gt(splat(x as u32));
            let miss = phase!(p, act);
            if miss.bitmask() == 0 {
                (w & act).store((&mut cur[s * NL..s * NL + NL]).try_into().unwrap());
                ww = w;
                p = if ahead {
                    spec
                } else {
                    look!(ctx!(s + 1, w, all))
                };
                ahead = true;
                continue;
            }
            cold();
            ahead = !md.adaptive;
            let mut val = w;
            let mut open = miss;
            let t = above(s);
            // Candidate rank `rk` with value `c`, tried by the open lanes where
            // it differs from W and the earlier candidates `q` (constant
            // indices throughout, so the template stays in registers).
            macro_rules! cand {
                ($rk:expr, $c:expr, [$($q:expr),*]) => {{
                    let c: u32x16 = $c;
                    let fresh = open $(& c.simd_ne($q))*;
                    if fresh.bitmask() != 0 {
                        // Template pixels equal to c (each equality is all ones).
                        let eq = c.simd_eq(t[0]) + c.simd_eq(t[1]) + c.simd_eq(t[2])
                            + c.simd_eq(t[3]) + c.simd_eq(t[4]) + c.simd_eq(t[5])
                            + c.simd_eq(t[6]) + c.simd_eq(t[7]) + c.simd_eq(ww)
                            + c.simd_eq(w);
                        let cnt = (zero - eq).min(splat(7));
                        let bk: &[u32; 8] = md.b[$rk * 8..$rk * 8 + 8].try_into().unwrap();
                        let mut pb = splat(bk[0]);
                        for j in 1..8 {
                            pb = u32x16::blend(cnt.simd_eq(splat(j as u32)), splat(bk[j]), pb);
                        }
                        let hit = fresh & phase!(pb, fresh).not();
                        val = u32x16::blend(hit, c, val);
                        open &= hit.not();
                    }
                }};
            }
            if let Some(rd) = &md.rank_dec {
                // Rank variant: one rank symbol for every open lane, in a
                // context of which candidates are fresh; escapes follow.
                let (c0, c1, c2, c3, c4) = (t[5], t[6], t[4], ww, t[1]);
                let f0 = c0.simd_ne(w);
                let f1 = c1.simd_ne(w) & c1.simd_ne(c0);
                let f2 = c2.simd_ne(w) & c2.simd_ne(c0) & c2.simd_ne(c1);
                let f3 = c3.simd_ne(w) & c3.simd_ne(c0) & c3.simd_ne(c1) & c3.simd_ne(c2);
                let f4 = c4.simd_ne(w)
                    & c4.simd_ne(c0)
                    & c4.simd_ne(c1)
                    & c4.simd_ne(c2)
                    & c4.simd_ne(c3);
                let ctx = (f0 & splat(1))
                    | (f1 & splat(2))
                    | (f2 & splat(4))
                    | (f3 & splat(8))
                    | (f4 & splat(16));
                let at = ctx.shl_const::<10>() | (xs & splat(M - 1));
                let e = u32x16::from_array_t(
                    token,
                    at.to_array()
                        .map(|i| rd[i as usize & (RANK_CTX * M as usize - 1)]),
                );
                let nx = (e.shr_logical_const::<8>() & splat(0x7FF)) * xs.shr_logical_const::<10>()
                    + e.shr_logical_const::<19>();
                xs = u32x16::blend(open, nx, xs);
                let mut m = (open & xs.simd_lt(splat(RANS_L))).bitmask();
                if m != 0 {
                    let mut a = xs.to_array();
                    while m != 0 {
                        let i = m.trailing_zeros() as usize;
                        a[i] = (a[i] << 16) | words.word(wp) as u32;
                        wp += 1;
                        m &= m - 1;
                    }
                    xs = u32x16::from_array_t(token, a);
                }
                let sym = e & splat(0xFF);
                val = u32x16::blend(open & sym.simd_eq(zero), c0, val);
                val = u32x16::blend(open & sym.simd_eq(one), c1, val);
                val = u32x16::blend(open & sym.simd_eq(splat(2)), c2, val);
                val = u32x16::blend(open & sym.simd_eq(splat(3)), c3, val);
                val = u32x16::blend(open & sym.simd_eq(splat(4)), c4, val);
                open &= sym.simd_eq(splat(RANKS as u32 - 1));
            } else {
                // Candidates N, NE, NW, WW, NN (CAND), stopping once every lane
                // is resolved.
                cand!(0, t[5], [w]);
                if open.bitmask() != 0 {
                    cand!(1, t[6], [w, t[5]]);
                    if open.bitmask() != 0 {
                        cand!(2, t[4], [w, t[5], t[6]]);
                        if open.bitmask() != 0 {
                            cand!(3, ww, [w, t[5], t[6], t[4]]);
                            if open.bitmask() != 0 {
                                cand!(4, t[1], [w, t[5], t[6], t[4], ww]);
                            }
                        }
                    }
                }
            }
            if open.bitmask() != 0 && md.lane_escape {
                // Escapes lane by lane (the first decoder's way): only the open
                // lanes look up, in lane order.
                let mut m = open.bitmask();
                let mut xa = xs.to_array();
                let mut va = val.to_array();
                while m != 0 {
                    let i = m.trailing_zeros() as usize;
                    let e = md.dec[(xa[i] & (M - 1)) as usize];
                    xa[i] = ((e >> 8) & 0x7FF) * (xa[i] >> LOG_M) + (e >> 19);
                    rans::renorm_one(&mut xa[i], &mut wp, words);
                    va[i] = e & 0xFF;
                    m &= m - 1;
                }
                xs = u32x16::from_array_t(token, xa);
                val = u32x16::from_array_t(token, va);
            } else if open.bitmask() != 0 {
                // Escapes: every open lane looks its slot up at once, then
                // refills in lane order (the words' order).
                let e = u32x16::from_array_t(
                    token,
                    (xs & splat(M - 1))
                        .to_array()
                        .map(|i| md.dec[i as usize & (M as usize - 1)]),
                );
                let nx = (e.shr_logical_const::<8>() & splat(0x7FF)) * xs.shr_logical_const::<10>()
                    + e.shr_logical_const::<19>();
                xs = u32x16::blend(open, nx, xs);
                val = u32x16::blend(open, e & splat(0xFF), val);
                let mut m = (open & xs.simd_lt(splat(RANS_L))).bitmask();
                if m != 0 {
                    let mut a = xs.to_array();
                    while m != 0 {
                        let i = m.trailing_zeros() as usize;
                        a[i] = (a[i] << 16) | words.word(wp) as u32;
                        wp += 1;
                        m &= m - 1;
                    }
                    xs = u32x16::from_array_t(token, a);
                }
            }
            (val & act).store((&mut cur[s * NL..s * NL + NL]).try_into().unwrap());
            p = look!(ctx!(s + 1, val, w.simd_eq(val)));
            ww = w;
            w = val;
        }
        st.x = xs.to_array();
        st.wp = wp;
    }

    pub(super) fn row(
        md: &FastModel,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
    ) {
        if crate::v1::force_scalar() {
            return row_impl_scalar(ScalarToken, md, words, st, widths, lw, p2, p1, cur);
        }
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        if !crate::v1::force_avx2()
            && let Some(tok) = archmage::X64V4Token::summon()
        {
            return row_impl_v4(tok, md, words, st, widths, lw, p2, p1, cur);
        }
        incant!(
            row_impl(md, words, st, widths, lw, p2, p1, cur),
            [v3, neon, wasm128, scalar]
        );
    }

    /// Every tier this host runs, for the tests.
    #[cfg(all(test, feature = "encode"))]
    pub(super) fn tiers() -> Vec<(&'static str, FastRow)> {
        let mut v: Vec<(&'static str, FastRow)> = vec![("scalar", |a, b, c, d, e, f, g, h| {
            row_impl_scalar(ScalarToken, a, b, c, d, e, f, g, h)
        })];
        #[cfg(target_arch = "x86_64")]
        if X64V3Token::summon().is_some() {
            v.push(("v3", |a, b, c, d, e, f, g, h| {
                row_impl_v3(X64V3Token::summon().unwrap(), a, b, c, d, e, f, g, h)
            }));
        }
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        if X64V4Token::summon().is_some() {
            v.push(("v4", |a, b, c, d, e, f, g, h| {
                row_impl_v4(X64V4Token::summon().unwrap(), a, b, c, d, e, f, g, h)
            }));
        }
        #[cfg(target_arch = "aarch64")]
        if NeonToken::summon().is_some() {
            v.push(("neon", |a, b, c, d, e, f, g, h| {
                row_impl_neon(NeonToken::summon().unwrap(), a, b, c, d, e, f, g, h)
            }));
        }
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        v.push(("wasm128", |a, b, c, d, e, f, g, h| {
            row_impl_wasm128(Wasm128Token::summon().unwrap(), a, b, c, d, e, f, g, h)
        }));
        v
    }
}

/// Where each halo step of a row comes from: `(destination, source)` row
/// buffer indices, `None` for columns outside the image (value 0). Fixed per
/// unit geometry, so the per-row fill needs no division.
fn halo_plan(w: usize, lw: usize) -> Vec<(usize, Option<usize>)> {
    let src = |col: isize| -> Option<usize> {
        (col >= 0 && (col as usize) < w).then(|| (col as usize % lw + 2) * NL + col as usize / lw)
    };
    let mut v = Vec::with_capacity(4 * NL);
    for l in 0..NL {
        let lane0 = (l * lw) as isize;
        for k in 0..2 {
            v.push((k * NL + l, src(lane0 - 2 + k as isize)));
            v.push(((lw + 2 + k) * NL + l, src(lane0 + lw as isize + k as isize)));
        }
    }
    v
}

/// Writes one decoded row (step-major indices) as pixels through the
/// palette table, lane by lane, eight pixels per iteration.
fn emit_row<const C: usize>(
    out: &mut [u8],
    cur: &[u32],
    widths: &[u32; NL],
    lw: usize,
    lut: &[[u8; C]; 256],
) {
    let out = out.as_chunks_mut::<C>().0;
    for (l, &wl) in widths.iter().enumerate() {
        let wl = wl as usize;
        if wl == 0 {
            break;
        }
        let o = &mut out[l * lw..l * lw + wl];
        let src = &cur[2 * NL + l..];
        let (blocks, tail) = o.as_chunks_mut::<8>();
        for (b, ob) in blocks.iter_mut().enumerate() {
            let sb: &[u32; 8 * NL - (NL - 1)] =
                src[b * 8 * NL..b * 8 * NL + 7 * NL + 1].try_into().unwrap();
            for (k, o) in ob.iter_mut().enumerate() {
                *o = lut[sb[k * NL] as u8 as usize];
            }
        }
        let done = blocks.len() * 8;
        for (k, o) in tail.iter_mut().enumerate() {
            *o = lut[src[(done + k) * NL] as u8 as usize];
        }
    }
}

/// The palette as pixel bytes per index (256 entries; indices from n up
/// repeat entry n - 1, as [`decode_with`]'s table).
fn lut256<const C: usize>(pal: &[u8], n: usize) -> alloc::boxed::Box<[[u8; C]; 256]> {
    alloc::boxed::Box::new(core::array::from_fn(|i| {
        let j = (i & 31).min(n - 1) * C;
        core::array::from_fn(|c| pal[j + c])
    }))
}

type FastRow =
    fn(&FastModel, &Words<'_>, &mut LaneState, &[u32; NL], usize, &[u32], &[u32], &mut [u32]);

/// The optimized decoder of one unit: decodes rows with `row` and writes
/// their pixels (`out`: the unit's `rows * w * ch` bytes) directly.
#[allow(clippy::too_many_arguments)]
fn decode_unit_fast(
    row: FastRow,
    md: &FastModel,
    bytes: &[u8],
    w: usize,
    rows: usize,
    lw: usize,
    ch: usize,
    pal: &[u8],
    typical: &[bool],
    out: &mut [u8],
) -> Result<(), Error> {
    let widths = crate::v1::lane_widths(w, lw);
    let (words, x) = Words::from_bytes(bytes, crate::v1::active_lanes(&widths))?;
    let n = md.n;
    let (l1, l2, l3, l4) = match ch {
        1 => (Some(lut256::<1>(pal, n)), None, None, None),
        2 => (None, Some(lut256::<2>(pal, n)), None, None),
        3 => (None, None, Some(lut256::<3>(pal, n)), None),
        _ => (None, None, None, Some(lut256::<4>(pal, n))),
    };
    let mut st = LaneState {
        x,
        run: [0; NL],
        wp: 0,
    };
    let nsteps = lw + 5;
    let mut b = [
        vec![0u32; nsteps * NL],
        vec![0u32; nsteps * NL],
        vec![0u32; nsteps * NL],
    ];
    let plan = halo_plan(w, lw);
    let rb = w * ch;
    for r in 0..rows {
        let [b0, b1, b2] = &mut b;
        let (p2, p1, cur) = match r % 3 {
            0 => (&*b1, &*b2, b0),
            1 => (&*b2, &*b0, b1),
            _ => (&*b0, &*b1, b2),
        };
        let (done, rest) = out.split_at_mut(r * rb);
        let orow = &mut rest[..rb];
        if typical[r] {
            // The row above, halo steps included; no symbols. Below the
            // first row its pixels are already written.
            cur.copy_from_slice(p1);
            if r > 0 {
                orow.copy_from_slice(&done[(r - 1) * rb..]);
                continue;
            }
        } else {
            // Current-row pixels left of a lane read the row above.
            cur[..2 * NL].copy_from_slice(&p1[..2 * NL]);
            row(md, &words, &mut st, &widths, lw, p2, p1, cur);
            for &(d, s) in &plan {
                cur[d] = s.map_or(0, |s| cur[s]);
            }
        }
        if let Some(t) = &l1 {
            emit_row(orow, cur, &widths, lw, t);
        } else if let Some(t) = &l2 {
            emit_row(orow, cur, &widths, lw, t);
        } else if let Some(t) = &l3 {
            emit_row(orow, cur, &widths, lw, t);
        } else if let Some(t) = &l4 {
            emit_row(orow, cur, &widths, lw, t);
        }
    }
    if st.wp > words.len {
        return Err(Error::Corrupt("lane stream overrun"));
    }
    Ok(())
}

/// A unit's row bitmap (all clear without typical rows) and the rest of its
/// bytes. Unused high bits must be zero.
fn typical_rows(tp: bool, bytes: &[u8], rows: usize) -> Result<(Vec<bool>, &[u8]), Error> {
    if !tp {
        return Ok((vec![false; rows], bytes));
    }
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

/// Which decoder [`decode_with`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoder {
    /// Phases in order, pixel by pixel (`decode_unit_ref`).
    Reference,
    /// The study's first vector decoder (lowbit_2026-10-11).
    Vector,
    /// Fixed-size tables, registers for W/WW, lookahead, fused output.
    Fast,
    /// [`Decoder::Fast`] preparing the next step only after a step where no
    /// lane missed (no wasted lookup in runs of misses).
    FastAdaptive,
    /// [`Decoder::FastAdaptive`] with escapes decoded one open lane at a
    /// time.
    FastAdaptiveLaneEscape,
}

fn expand_px<const C: usize>(out: &mut [u8], idx: &[u8], lut: &[[u8; 4]; 32]) {
    for (o, &i) in out.as_chunks_mut::<C>().0.iter_mut().zip(idx) {
        o.copy_from_slice(&lut[i as usize & 31][..C]);
    }
}

/// Decodes into `out` (`w * h * ch` bytes): `vec` the fast vector decoder,
/// else the reference. Returns `(w, h, ch)`.
pub fn decode(
    file: &[u8],
    out: &mut [u8],
    threads: usize,
    vec: bool,
) -> Result<(usize, usize, usize), Error> {
    let d = if vec {
        Decoder::Fast
    } else {
        Decoder::Reference
    };
    decode_with(file, out, threads, d)
}

/// [`decode`] with the decoder chosen.
pub fn decode_with(
    file: &[u8],
    out: &mut [u8],
    threads: usize,
    dec: Decoder,
) -> Result<(usize, usize, usize), Error> {
    let hd = parse(file)?;
    let (w, h, ch) = (hd.w, hd.h, hd.ch);
    if out.len() < w * h * ch {
        return Err(Error::InvalidInput("output too small"));
    }
    let lw = w.div_ceil(NL);
    let (md, pal, n, tp) = (&hd.md, hd.palette, hd.n, hd.tp);
    if !md.rank_freq.is_empty() && dec == Decoder::Vector {
        return Err(Error::Unsupported(
            "rank variant: not the first vector decoder",
        ));
    }
    let fast = matches!(
        dec,
        Decoder::Fast | Decoder::FastAdaptive | Decoder::FastAdaptiveLaneEscape
    )
    .then(|| {
        FastModel::new(
            md,
            dec != Decoder::Fast,
            dec == Decoder::FastAdaptiveLaneEscape,
        )
    });
    let items: Vec<(&[u8], &mut [u8])> = hd
        .units
        .iter()
        .copied()
        .zip(out[..w * h * ch].chunks_mut(hd.hs * w * ch))
        .collect();
    // Each unit decodes its indices and expands them to pixels itself.
    for res in crate::par::par_map(items, threads, |_, (bytes, chunk)| {
        let rows = chunk.len() / (w * ch);
        let (typical, bytes) = typical_rows(tp, bytes, rows)?;
        if let Some(f) = &fast {
            return decode_unit_fast(
                fast_row::row,
                f,
                bytes,
                w,
                rows,
                lw,
                ch,
                pal,
                &typical,
                chunk,
            );
        }
        let mut lut = [[0u8; 4]; 32];
        for (i, e) in lut.iter_mut().enumerate() {
            let j = i.min(n - 1) * ch;
            e[..ch].copy_from_slice(&pal[j..j + ch]);
        }
        let mut idx = vec![0u8; rows * w];
        if dec == Decoder::Vector {
            decode_unit_vec(md, bytes, w, rows, lw, &typical, &mut idx)?;
        } else {
            decode_unit_ref(md, bytes, w, rows, lw, &typical, &mut idx)?;
        }
        match ch {
            1 => chunk
                .iter_mut()
                .zip(&idx)
                .for_each(|(o, &i)| *o = lut[i as usize & 31][0]),
            2 => expand_px::<2>(chunk, &idx, &lut),
            3 => expand_px::<3>(chunk, &idx, &lut),
            _ => expand_px::<4>(chunk, &idx, &lut),
        }
        Ok::<(), Error>(())
    }) {
        res?;
    }
    Ok((w, h, ch))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn image(w: usize, h: usize, n: u32, seed: u64) -> Vec<u8> {
        let mut s = seed | 1;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let mut v = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let c = if next() % 5 == 0 {
                    (next() % n as u64) as u32
                } else {
                    ((x / 4 + y / 3) as u32 * 7) % n
                };
                v[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[
                    (c as u8).wrapping_mul(9),
                    255 - c as u8,
                    3,
                ]);
            }
        }
        v
    }

    /// Every unit through every fast-kernel tier, the first vector decoder and
    /// the reference: the same pixels (no candidate or escape symbol is
    /// decoded differently; the files are the encoder's, unchanged).
    #[test]
    fn fast_tiers_match_the_reference() {
        let mut checked = 0;
        for n in [2u32, 3, 4, 5, 9, 17, 32] {
            for &(w, h) in &[
                (1, 1),
                (2, 3),
                (3, 7),
                (16, 5),
                (17, 9),
                (31, 4),
                (40, 33),
                (65, 20),
                (233, 21),
                (515, 9),
            ] {
                let px = image(w, h, n, (n as u64) * 7919 + w as u64 * 13 + h as u64);
                let Some(f) = encode(&px, w, h, 3, 2) else {
                    continue;
                };
                let hd = parse(&f).unwrap();
                let fms = [
                    FastModel::new(&hd.md, false, false),
                    FastModel::new(&hd.md, true, false),
                    FastModel::new(&hd.md, true, true),
                ];
                let lw = w.div_ceil(NL);
                for (u, bytes) in hd.units.iter().enumerate() {
                    let y0 = u * hd.hs;
                    let rows = hd.hs.min(h - y0);
                    let want = &px[y0 * w * 3..(y0 + rows) * w * 3];
                    for (name, row) in fast_row::tiers() {
                        for fm in &fms {
                            let mut got = vec![0u8; rows * w * 3];
                            let typical = vec![false; rows];
                            decode_unit_fast(
                                row, fm, bytes, w, rows, lw, 3, hd.palette, &typical, &mut got,
                            )
                            .unwrap();
                            let mode = (fm.adaptive, fm.lane_escape);
                            assert_eq!(got, want, "{name} n {n} {w}x{h} unit {u} {mode:?}");
                            checked += 1;
                        }
                    }
                }
                for d in [
                    Decoder::Reference,
                    Decoder::Vector,
                    Decoder::Fast,
                    Decoder::FastAdaptive,
                    Decoder::FastAdaptiveLaneEscape,
                ] {
                    for th in [1, 3] {
                        let mut out = vec![0u8; px.len()];
                        decode_with(&f, &mut out, th, d).unwrap();
                        assert_eq!(out, px, "{d:?} n {n} {w}x{h} threads {th}");
                    }
                }
            }
        }
        assert!(checked > 0);
    }

    /// Images where rows repeat, every color stays except rare escapes, and
    /// four-color runs: the lookahead's hit and miss paths both run.
    #[test]
    fn fast_matches_reference_on_runs_and_escapes() {
        for n in [3u32, 6, 32] {
            for &(w, h) in &[(64, 40), (100, 17), (300, 70)] {
                let mut px = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        let c = if (x * 31 + y * 17) % 97 == 0 {
                            ((x + y) as u32) % n
                        } else if y % 9 < 3 {
                            0
                        } else {
                            ((x / 7) as u32) % n.min(4)
                        };
                        px[y * w + x] = c as u8 * 7;
                    }
                }
                // Make sure every color occurs.
                for c in 0..n as usize {
                    px[c] = c as u8 * 7;
                }
                let f = encode(&px, w, h, 1, 1).unwrap();
                for d in [
                    Decoder::Reference,
                    Decoder::Vector,
                    Decoder::Fast,
                    Decoder::FastAdaptive,
                    Decoder::FastAdaptiveLaneEscape,
                ] {
                    let mut out = vec![0u8; px.len()];
                    decode_with(&f, &mut out, 2, d).unwrap();
                    assert_eq!(out, px, "{d:?} n {n} {w}x{h}");
                }
            }
        }
    }

    /// Typical rows: every decoder and fast-kernel tier reproduces images
    /// whose units start blank, repeat rows in runs, repeat whole units or
    /// never repeat; a bitmap with padding bits set is rejected.
    #[test]
    fn typical_rows_every_decoder_and_tier() {
        let mut checked = 0;
        for n in [2u32, 3, 7, 32] {
            for &(w, h) in &[(1, 1), (5, 9), (17, 40), (64, 70), (233, 37), (515, 20)] {
                let mut px = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        let band = (y / 5) % 4;
                        let c = match band {
                            0 => 0,
                            1 => ((x / 3) as u32 % n) as u8,
                            2 => (((x * 7 + y / 5 * 3) / 4) as u32 % n) as u8,
                            _ => ((x * 31 + y * 17) as u32 % n) as u8,
                        };
                        px[y * w + x] = c * 5;
                    }
                }
                for c in 0..(n as usize).min(w * h) {
                    px[c] = c as u8 * 5;
                }
                let Some(f) = encode_typical_rows(&px, w, h, 1, 2) else {
                    continue;
                };
                assert_eq!(&f[..6], MAGIC_TP);
                for d in [
                    Decoder::Reference,
                    Decoder::Vector,
                    Decoder::Fast,
                    Decoder::FastAdaptive,
                    Decoder::FastAdaptiveLaneEscape,
                ] {
                    for th in [1, 3] {
                        let mut out = vec![0u8; px.len()];
                        decode_with(&f, &mut out, th, d).unwrap();
                        assert_eq!(out, px, "{d:?} n {n} {w}x{h} threads {th}");
                    }
                }
                let hd = parse(&f).unwrap();
                let fm = FastModel::new(&hd.md, true, false);
                let lw = w.div_ceil(NL);
                for (u, bytes) in hd.units.iter().enumerate() {
                    let y0 = u * hd.hs;
                    let rows = hd.hs.min(h - y0);
                    let (typical, rest) = typical_rows(true, bytes, rows).unwrap();
                    let want = &px[y0 * w..(y0 + rows) * w];
                    for (name, row) in fast_row::tiers() {
                        let mut got = vec![0u8; rows * w];
                        decode_unit_fast(
                            row, &fm, rest, w, rows, lw, 1, hd.palette, &typical, &mut got,
                        )
                        .unwrap();
                        assert_eq!(got, want, "{name} n {n} {w}x{h} unit {u}");
                        checked += 1;
                    }
                    if !rows.is_multiple_of(8) {
                        let mut bad = bytes.to_vec();
                        bad[rows / 8] |= 0x80;
                        assert!(typical_rows(true, &bad, rows).is_err());
                    }
                }
            }
        }
        assert!(checked > 0);
    }

    /// The rank variant, without and with typical rows: the reference and
    /// every fast decoder (and fast-kernel tier) reproduce every image.
    #[test]
    fn rank_variant_roundtrip() {
        for n in [2u32, 3, 5, 9, 17, 32] {
            for &(w, h) in &[(1, 1), (3, 7), (17, 9), (40, 33), (233, 21), (300, 70)] {
                let px = image(w, h, n, (n as u64) * 31 + w as u64 * 3 + h as u64);
                for tp in [false, true] {
                    let Some(f) = encode_rank(&px, w, h, 3, 2, tp) else {
                        continue;
                    };
                    assert_eq!(&f[..6], if tp { MAGIC_RANK_TP } else { MAGIC_RANK });
                    for d in [
                        Decoder::Reference,
                        Decoder::Fast,
                        Decoder::FastAdaptive,
                        Decoder::FastAdaptiveLaneEscape,
                    ] {
                        for th in [1, 3] {
                            let mut out = vec![0u8; px.len()];
                            decode_with(&f, &mut out, th, d).unwrap();
                            assert_eq!(out, px, "{d:?} n {n} {w}x{h} tp {tp} threads {th}");
                        }
                    }
                    let hd = parse(&f).unwrap();
                    let fm = FastModel::new(&hd.md, true, false);
                    let lw = w.div_ceil(NL);
                    for (u, bytes) in hd.units.iter().enumerate() {
                        let y0 = u * hd.hs;
                        let rows = hd.hs.min(h - y0);
                        let (typical, rest) = typical_rows(tp, bytes, rows).unwrap();
                        let want = &px[y0 * w * 3..(y0 + rows) * w * 3];
                        for (name, row) in fast_row::tiers() {
                            let mut got = vec![0u8; rows * w * 3];
                            decode_unit_fast(
                                row, &fm, rest, w, rows, lw, 3, hd.palette, &typical, &mut got,
                            )
                            .unwrap();
                            assert_eq!(got, want, "{name} n {n} {w}x{h} tp {tp} unit {u}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn roundtrip_reference_and_vector() {
        for n in [2u32, 3, 5, 9, 17, 32] {
            for &(w, h) in &[(1, 1), (3, 7), (16, 5), (17, 9), (40, 33), (233, 21)] {
                let px = image(w, h, n, (n as u64) * 1000 + w as u64 * 7 + h as u64);
                let Some(f) = encode(&px, w, h, 3, 2) else {
                    continue;
                };
                for vec in [false, true] {
                    let mut out = vec![0u8; px.len()];
                    decode(&f, &mut out, 3, vec).unwrap();
                    assert_eq!(out, px, "n {n} {w}x{h} vec {vec}");
                }
            }
        }
    }
}
