#![allow(dead_code, unused_mut, unused_variables, clippy::all)]
//! Lab: a binary template-context rANS slot coder for 1-bit images
//! (`lab` feature only, not part of the format).
//!
//! Geometry and machinery are the lossless coder's: one slot per stripe,
//! 16 lanes of contiguous column segments (`v1::lane_widths`), a 32-bit
//! rANS state per lane in 1/1024 probabilities, renormalization words in
//! `(step, phase, lane)` decode order. The model is the only new part:
//!
//! * every pixel is one binary symbol; its context is a causal pixel
//!   template (JBIG1's 10-pixel three-line template, or a 16-pixel
//!   JBIG2-shaped variant), one static probability per context per image
//!   (two-pass encoder, quantized to 1/1024, table bytes in the file);
//! * variant [`Variant::Runs`] additionally codes, in the all-white
//!   context (ctx 0), hybrid-uint run tokens (direct `rk = 8` tokens, one
//!   per white run of up to [`RUN_MAX`] pixels) beside the two pixel bits.
//!
//! Neighbours left of a lane's segment on the current row belong to the
//! previous lane and are not decoded yet when the lane reaches that column,
//! so the coder substitutes the pixel above them (`row - 1`, same column);
//! rows above a lane's own row are fully decoded, and pixels above the
//! stripe read white. Both encoder and decoder apply the same rule.
//!
//! Shared with the lossless coder: `rans::LaneState`, `rans::Words` and
//! `rans::renorm_one` (lane state and renormalization, decode word order),
//! `crate::v1::lane_widths` / `crate::v1::active_lanes` /
//! `crate::v1::default_stripe_rows` (real stripe and lane geometry),
//! `crate::bits::BitWriter` / `crate::bits::BitReader` and the weight
//! coder (`rans::write_weights` / `rans::read_weights`,
//! `rans::weights_to_freq`, `rans::choose_table`) for the ctx-0 run table.
//! The encoder keeps its own symbol record and frequency tables (`BSym`,
//! `BEnc`) and its own word merge (`encode_lanes_bsym`); the default
//! coder's `rans::Sym` and `rans::EncodeTables` keep their main types
//! (16-bit contexts, flat equal-alphabet indexing), so the default build
//! pays nothing for this lab mode.

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::{BitReader, BitWriter};
use crate::rans::{self, LOG_M, LaneState, M, NL, Words};

/// JBIG1's 10-pixel three-line template, bit `i` = `T10[i]` (1 = black),
/// matching `experiments/pages_2026-10-08/bilevel_context.py` T10.
pub const T10: [(i8, i8); 10] = [
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

/// 16-pixel variant shaped like JBIG2 generic template 0, same convention.
pub const T16: [(i8, i8); 16] = [
    (-2, -2),
    (-2, -1),
    (-2, 0),
    (-2, 1),
    (-2, 2),
    (-1, -3),
    (-1, -2),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (-1, 2),
    (-1, 3),
    (0, -4),
    (0, -3),
    (0, -2),
    (0, -1),
];

/// Longest white run one run token codes (`2 + run_max - 1 == 255`).
pub const RUN_MAX: u32 = 254;
/// Value/bit and run tokens: 0 white, 1 black, `2 + (len - 1)` runs.
const MIXED_SYMS: usize = 256;
const MAGIC: &[u8; 6] = b"IMIJB1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    /// One binary symbol per pixel in every context.
    Bits,
    /// Binary symbols except in ctx 0, where run tokens are also coded.
    Runs,
}

#[derive(Clone, Copy, Debug)]
pub struct Cfg {
    pub variant: Variant,
    /// Use the 16-pixel template instead of JBIG1's 10.
    pub template16: bool,
    /// Shortest white run coded as a run token (variant `Runs`).
    pub run_min: u32,
    /// Rows per stripe; `None` uses the lossless default.
    pub stripe_rows: Option<u32>,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            variant: Variant::Bits,
            template16: false,
            run_min: 4,
            stripe_rows: None,
        }
    }
}

fn template_of(cfg: &Cfg) -> &'static [(i8, i8)] {
    if cfg.template16 { &T16 } else { &T10 }
}

/// The image's static model: one probability (in 1/1024) per context, plus
/// the ctx-0 run table for [`Variant::Runs`].
pub struct Model {
    pub nctx: usize,
    /// P(symbol 0) in 1/1024; only consulted when `mixed` does not claim
    /// the context (ctx 0 under `Runs`).
    pub f0: Vec<u16>,
    /// One bit per context: written in the file (only used contexts pay
    /// table bytes; the 16-pixel table is sparse).
    pub used: Vec<u8>,
    pub mixed: Option<Mixed>,
}

/// Ctx-0 mixed alphabet for `Runs`: 256 symbols, entries in the lossless
/// decode-table layout `(freq - 1) | bias << 10 | sym << 20`.
pub struct Mixed {
    pub weights: Vec<u32>,
    pub prec: u32,
    pub freq: Vec<u32>,
    pub dec0: Vec<u32>,
}

#[inline]
fn quant_p0(c0: u64, c1: u64) -> u16 {
    let t = c0 + c1;
    if t == 0 {
        return (M / 2) as u16;
    }
    let q = ((c0 * M as u64 + t / 2) / t).min(M as u64) as u32;
    let q = if c0 > 0 && q == 0 {
        1
    } else if c1 > 0 && q == M {
        M - 1
    } else {
        q
    };
    q as u16
}

fn mixed_from_weights(wt: &[u32], prec: u32) -> Option<Mixed> {
    let freq = rans::weights_to_freq(wt)?;
    let mut cum = vec![0u32; MIXED_SYMS + 1];
    for s in 0..MIXED_SYMS {
        cum[s + 1] = cum[s] + freq[s];
    }
    if cum[MIXED_SYMS] != M {
        return None;
    }
    let mut dec0 = vec![0u32; M as usize];
    for sym in 0..MIXED_SYMS {
        let f = freq[sym];
        if f == 0 {
            continue;
        }
        for slot in cum[sym]..cum[sym + 1] {
            dec0[slot as usize] = (f - 1) | ((slot - cum[sym]) << 10) | ((sym as u32) << 20);
        }
    }
    Some(Mixed {
        weights: wt.to_vec(),
        prec,
        freq,
        dec0,
    })
}

/// Running per-context counts of one stripe (pass 1).
struct Hist {
    c0: Vec<u64>,
    c1: Vec<u64>,
    mixed: Vec<u64>,
}

fn build_model(cfg: &Cfg, nctx: usize, h0: &[u64], h1: &[u64], hm: &[u64]) -> Model {
    let mut f0 = vec![(M / 2) as u16; nctx];
    let mut used = vec![0u8; nctx / 8];
    let mixed = if cfg.variant == Variant::Runs {
        let mut counts = hm.to_vec();
        if counts.iter().all(|&c| c == 0) {
            counts[0] = 1;
        }
        let (wt, _f, prec) = rans::choose_table(&counts);
        Some(mixed_from_weights(&wt, prec).expect("choose_table yields a normalizable table"))
    } else {
        None
    };
    for c in 0..nctx {
        let occurs = h0[c] + h1[c] > 0;
        if occurs {
            used[c >> 3] |= 1 << (c & 7);
        }
        if cfg.variant == Variant::Bits || c != 0 {
            f0[c] = quant_p0(h0[c], h1[c]);
        }
    }
    if cfg.variant == Variant::Runs && hm.iter().sum::<u64>() > 0 {
        used[0] |= 1;
    }
    Model {
        nctx,
        f0,
        used,
        mixed,
    }
}

/// Per-context frequency tables for `BEnc`: binary `[f0, M - f0]`
/// everywhere except ctx 0 under `Runs`, which uses the 256-symbol mixed
/// table.
fn model_freqs(md: &Model, variant: Variant) -> Vec<Vec<u32>> {
    (0..md.nctx)
        .map(|c| {
            if variant == Variant::Runs && c == 0 {
                md.mixed.as_ref().unwrap().freq.clone()
            } else {
                let f0 = md.f0[c] as u32;
                vec![f0, M - f0]
            }
        })
        .collect()
}

// ---- context computation ---------------------------------------------------

#[inline(always)]
fn col_read(plane: &[u16], w: usize, lw: usize, row: usize, col: usize) -> u16 {
    if col >= w {
        return 0;
    }
    let lane = col / lw;
    if lane >= NL {
        return 0;
    }
    plane[(row * lw + col % lw) * NL + lane]
}

/// Context of lane `l`, step `x`, stripe row `row`, read from the decoded
/// step-major stripe plane. Current-row neighbours left of the lane's
/// segment (not decoded yet: previous lane's columns) read the row above at
/// the same column; row 0 substitutes 0. `dy < 0` rows read the plane
/// (rows above the stripe are 0). Columns outside `w` read 0.
fn ctx_dec(
    plane: &[u16],
    w: usize,
    lw: usize,
    row: usize,
    x: usize,
    l: usize,
    tpl: &[(i8, i8)],
) -> u16 {
    let lane0 = (l * lw) as isize;
    let mut ctx = 0u16;
    for (i, &(dy, dx)) in tpl.iter().enumerate() {
        let bit = if dy == 0 {
            let xs = x as isize + dx as isize;
            if xs >= 0 {
                col_read(plane, w, lw, row, (lane0 + xs) as usize)
            } else if row == 0 || lane0 + xs < 0 {
                0
            } else {
                col_read(plane, w, lw, row - 1, (lane0 + xs) as usize)
            }
        } else {
            let rr = row as isize + dy as isize;
            let col = lane0 + x as isize + dx as isize;
            if rr < 0 || col < 0 {
                0
            } else {
                col_read(plane, w, lw, rr as usize, col as usize)
            }
        };
        ctx |= ((bit != 0) as u16) << i;
    }
    ctx
}

#[inline(always)]
fn src_read(src: &[u8], w: usize, y0: usize, row: usize, col: usize) -> u8 {
    if col >= w {
        0
    } else {
        src[(y0 + row) * w + col]
    }
}

/// Encoder-side twin of [`ctx_dec`]: the same rule, reading the source.
fn ctx_enc(
    src: &[u8],
    w: usize,
    y0: usize,
    row: usize,
    x: usize,
    l: usize,
    lw: usize,
    tpl: &[(i8, i8)],
) -> u16 {
    let lane0 = (l * lw) as isize;
    let mut ctx = 0u16;
    for (i, &(dy, dx)) in tpl.iter().enumerate() {
        let bit = if dy == 0 {
            let xs = x as isize + dx as isize;
            if xs >= 0 {
                src_read(src, w, y0, row, (lane0 + xs) as usize)
            } else if row == 0 || lane0 + xs < 0 {
                0
            } else {
                src_read(src, w, y0, row - 1, (lane0 + xs) as usize)
            }
        } else {
            let rr = row as isize + dy as isize;
            let col = lane0 + x as isize + dx as isize;
            if rr < 0 || col < 0 {
                0
            } else {
                src_read(src, w, y0, rr as usize, col as usize)
            }
        };
        ctx |= ((bit != 0) as u16) << i;
    }
    ctx
}

// ---- encoder walk ----------------------------------------------------------

/// One walk of lane `l` of one stripe in decode order, emitting
/// `(step, ctx, symbol)` per coded symbol. With `Runs`, a white pixel in
/// ctx 0 with at least `run_min` white pixels left in the lane is one run
/// token `2 + (len - 1)` (len capped at [`RUN_MAX`]); every other position
/// is one bit.
#[allow(clippy::too_many_arguments)]
fn lane_walk<F: FnMut(u32, u16, u16)>(
    src: &[u8],
    w: usize,
    y0: usize,
    rows: usize,
    lw: usize,
    widths: &[u32; NL],
    tpl: &[(i8, i8)],
    cfg: &Cfg,
    l: usize,
    emit: &mut F,
) {
    let width = widths[l] as usize;
    if width == 0 {
        return;
    }
    let n = rows * width;
    let runs = cfg.variant == Variant::Runs && cfg.run_min >= 2 && cfg.run_min <= RUN_MAX;
    let mut i = 0usize;
    while i < n {
        let r = i / width;
        let x = i % width;
        let step = (r * lw + x) as u32;
        let ctx = ctx_enc(src, w, y0, r, x, l, lw, tpl);
        let black = src[(y0 + r) * w + l * lw + x] != 0;
        if runs && ctx == 0 && !black {
            let cap = (n - i).min(RUN_MAX as usize);
            let mut len = 1usize;
            while len < cap && src[(y0 + (i + len) / width) * w + l * lw + (i + len) % width] == 0 {
                len += 1;
            }
            if len as u32 >= cfg.run_min {
                emit(step, 0, (2 + len - 1) as u16);
                i += len;
                continue;
            }
        }
        emit(step, ctx, black as u16);
        i += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn stripe_hist(
    src: &[u8],
    w: usize,
    y0: usize,
    rows: usize,
    lw: usize,
    widths: &[u32; NL],
    tpl: &[(i8, i8)],
    cfg: &Cfg,
) -> Hist {
    let nctx = 1usize << tpl.len();
    let mut h = Hist {
        c0: vec![0u64; nctx],
        c1: vec![0u64; nctx],
        mixed: vec![0u64; MIXED_SYMS],
    };
    for l in 0..NL {
        lane_walk(
            src,
            w,
            y0,
            rows,
            lw,
            widths,
            tpl,
            cfg,
            l,
            &mut |_step, ctx, sym| {
                if ctx == 0 && cfg.variant == Variant::Runs {
                    h.mixed[sym as usize] += 1;
                } else if sym == 0 {
                    h.c0[ctx as usize] += 1;
                } else {
                    h.c1[ctx as usize] += 1;
                }
            },
        );
    }
    h
}

/// One coded symbol of a lane (bilevel encoder only): the template
/// context, and a token that is a pixel bit or a ctx-0 run symbol. There
/// is no extra-bit phase, so the record needs neither `nb` nor `extra`.
#[derive(Clone, Copy)]
struct BSym {
    step: u32,
    ctx: u16,
    tok: u8,
}

/// Encoder-only frequency tables for the bilevel model: template contexts
/// map 1:1 (up to 65536 of them), and ctx 0's run table is wider than the
/// binary tables, so each table's offset is stored. The default coder's
/// `rans::EncodeTables` keeps its smaller main types.
struct BEnc {
    offsets: Vec<u32>,
    symbols: Vec<BEncSymbol>,
}

struct BEncSymbol {
    freq: u32,
    cum: u32,
    recip: u64,
}

impl BEnc {
    fn new(freq: &[Vec<u32>]) -> Self {
        let total: usize = freq.iter().map(|t| t.len()).sum();
        let mut symbols = Vec::with_capacity(total);
        let mut offsets = Vec::with_capacity(freq.len());
        for table in freq {
            offsets.push(symbols.len() as u32);
            let mut cum = 0;
            symbols.extend(table.iter().map(|&freq| {
                let symbol = BEncSymbol {
                    freq,
                    cum,
                    // Exact division for 32-bit x: (x * ceil(2^64 / f)) >> 64.
                    recip: if freq <= 1 {
                        0
                    } else {
                        (u64::MAX / freq as u64) + 1
                    },
                };
                cum += freq;
                symbol
            }));
        }
        Self { offsets, symbols }
    }

    #[inline(always)]
    fn symbol(&self, ctx: usize, tok: usize) -> &BEncSymbol {
        &self.symbols[self.offsets[ctx] as usize + tok]
    }
}

/// Encoder twin of `rans::encode_lanes_tables` for `BSym` lists: the
/// active initial states, then renormalization words in decoder
/// consumption order `(step, lane)` (bilevel symbols have no extra-bit
/// phase). Only renormalizing symbols emit a word, so the 16 lane lists
/// are concatenated and sorted instead of merged.
fn encode_lanes_bsym(tables: &BEnc, lanes: &[Vec<BSym>; NL], active: usize, out: &mut Vec<u8>) {
    let mut states = [0u32; NL];
    let mut words: Vec<(u64, u16)> = Vec::new();
    for (l, syms) in lanes.iter().enumerate() {
        let mut x = rans::RANS_L;
        for y in syms.iter().rev() {
            let s = tables.symbol(y.ctx as usize, y.tok as usize);
            let f = s.freq;
            let x_max = (f as u64) << 22; // (RANS_L >> LOG_M) << 16
            if x as u64 >= x_max {
                words.push((((y.step as u64) << 4) | l as u64, x as u16));
                x >>= 16;
            }
            let q = if f == 1 {
                x
            } else {
                ((x as u128 * s.recip as u128) >> 64) as u32
            };
            debug_assert_eq!(q, x / f);
            x = (q << LOG_M) + (x - q * f) + s.cum;
        }
        states[l] = x;
    }
    out.reserve(4 * active + 2 * words.len());
    for &x in states.iter().take(active) {
        out.extend_from_slice(&x.to_le_bytes());
    }
    words.sort_unstable_by_key(|&(k, _)| k);
    for (_, w) in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
fn stripe_encode(
    src: &[u8],
    w: usize,
    y0: usize,
    rows: usize,
    lw: usize,
    widths: &[u32; NL],
    tpl: &[(i8, i8)],
    cfg: &Cfg,
    tables: &BEnc,
) -> Vec<u8> {
    let lanes: [Vec<BSym>; NL] = core::array::from_fn(|l| {
        let mut v = Vec::new();
        lane_walk(
            src,
            w,
            y0,
            rows,
            lw,
            widths,
            tpl,
            cfg,
            l,
            &mut |step, ctx, sym| {
                v.push(BSym {
                    step,
                    ctx,
                    tok: sym as u8,
                });
            },
        );
        v
    });
    let mut out = Vec::new();
    encode_lanes_bsym(tables, &lanes, crate::v1::active_lanes(widths), &mut out);
    out
}

// ---- envelope --------------------------------------------------------------

fn push_u16_bits(bw: &mut BitWriter, v: u32, bits: u32) {
    bw.put(v, bits);
}

fn write_model(out: &mut Vec<u8>, md: &Model) {
    let mut bw = BitWriter::with_capacity(md.nctx / 4);
    for chunk in md.used.chunks(4) {
        let mut word = 0u32;
        for (i, &b) in chunk.iter().enumerate() {
            word |= (b as u32) << (8 * i);
        }
        bw.put(word, 32);
    }
    for c in 0..md.nctx {
        if md.used[c >> 3] & (1 << (c & 7)) != 0 {
            push_u16_bits(&mut bw, md.f0[c] as u32, 11);
        }
    }
    if let Some(m) = &md.mixed {
        rans::write_weights(&mut bw, &m.weights, m.prec);
    }
    let tb = bw.finish();
    crate::put_varint(out, tb.len() as u64);
    out.extend_from_slice(&tb);
}

fn read_model(bytes: &[u8], variant: Variant, tpl_len: usize) -> Result<Model, Error> {
    let nctx = 1usize << tpl_len;
    let mut r = BitReader::new(bytes);
    let mut used = vec![0u8; nctx / 8];
    for wi in 0..nctx / 32 {
        let v = r.read_refill(32);
        for b in 0..4 {
            used[wi * 4 + b] = (v >> (8 * b)) as u8;
        }
    }
    let mut f0 = vec![(M / 2) as u16; nctx];
    for c in 0..nctx {
        if used[c >> 3] & (1 << (c & 7)) != 0 {
            let v = r.read_refill(11);
            if v > M {
                return Err(Error::Corrupt("bilevel context probability out of range"));
            }
            f0[c] = v as u16;
        }
    }
    let mixed = if variant == Variant::Runs {
        let (wt, prec) = rans::read_weights(&mut r, MIXED_SYMS)?;
        Some(mixed_from_weights(&wt, prec).ok_or(Error::Corrupt("empty run table"))?)
    } else {
        None
    };
    if r.overrun() {
        return Err(Error::Corrupt("bilevel table overruns"));
    }
    Ok(Model {
        nctx,
        f0,
        used,
        mixed,
    })
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Result<u8, Error> {
        let v = *self
            .b
            .get(self.p)
            .ok_or(Error::Corrupt("truncated bilevel file"))?;
        self.p += 1;
        Ok(v)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.p.checked_add(n).ok_or(Error::Corrupt("bad length"))?;
        let v = self
            .b
            .get(self.p..end)
            .ok_or(Error::Corrupt("truncated bilevel file"))?;
        self.p = end;
        Ok(v)
    }

    fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for i in 0..10 {
            let b = self.u8()?;
            if i == 9 && b > 1 {
                return Err(Error::Corrupt("varint overflow"));
            }
            value |= ((b & 0x7f) as u64) << (7 * i);
            if b < 0x80 {
                return Ok(value);
            }
        }
        Err(Error::Corrupt("bad varint"))
    }
}

struct Head {
    w: usize,
    h: usize,
    hs: usize,
    variant: Variant,
    tpl_len: usize,
}

fn parse_head(r: &mut Rd<'_>) -> Result<Head, Error> {
    if r.bytes(MAGIC.len())? != MAGIC {
        return Err(Error::Corrupt("not a bilevel lab file"));
    }
    let variant = match r.u8()? {
        0 => Variant::Bits,
        1 => Variant::Runs,
        _ => return Err(Error::Corrupt("bad bilevel variant")),
    };
    let tpl_len = match r.u8()? {
        10 => 10,
        16 => 16,
        _ => return Err(Error::Corrupt("bad bilevel template")),
    };
    let w = r.varint()? as usize;
    let h = r.varint()? as usize;
    let hs = r.varint()? as usize;
    if w == 0 || h == 0 || hs == 0 || w > (1 << 24) || h > (1 << 24) {
        return Err(Error::Corrupt("bad bilevel dimensions"));
    }
    Ok(Head {
        w,
        h,
        hs,
        variant,
        tpl_len,
    })
}

/// Encodes a 1-bit plane (`plane[y * w + x]`, 1 = black) with the template
/// model. `threads` is used for the two parallel passes; encoding is
/// deterministic.
pub fn encode(plane: &[u8], w: usize, h: usize, cfg: &Cfg, threads: usize) -> Vec<u8> {
    assert!(w > 0 && h > 0 && plane.len() >= w * h);
    let tpl = template_of(cfg);
    let nctx = 1usize << tpl.len();
    let hs = cfg
        .stripe_rows
        .map(|v| (v as usize).max(1))
        .unwrap_or_else(|| crate::v1::default_stripe_rows(w as u32) as usize);
    let lw = w.div_ceil(NL);
    let widths = crate::v1::lane_widths(w, lw);
    let nstripes = h.div_ceil(hs);

    // Pass 1: context histograms over every stripe.
    let hists = crate::par::par_map(vec![0usize; nstripes], threads, |si, _| {
        let y0 = si * hs;
        let rows = (h - y0).min(hs);
        stripe_hist(plane, w, y0, rows, lw, &widths, tpl, cfg)
    });
    let mut h0 = vec![0u64; nctx];
    let mut h1 = vec![0u64; nctx];
    let mut hm = vec![0u64; MIXED_SYMS];
    for st in &hists {
        for c in 0..nctx {
            h0[c] += st.c0[c];
            h1[c] += st.c1[c];
        }
        for s in 0..MIXED_SYMS {
            hm[s] += st.mixed[s];
        }
    }
    let md = build_model(cfg, nctx, &h0, &h1, &hm);
    let freqs = model_freqs(&md, cfg.variant);
    let tables = BEnc::new(&freqs);

    // Pass 2: lane streams, one per stripe.
    let stripes = crate::par::par_map(vec![0usize; nstripes], threads, |si, _| {
        let y0 = si * hs;
        let rows = (h - y0).min(hs);
        stripe_encode(plane, w, y0, rows, lw, &widths, tpl, cfg, &tables)
    });

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(match cfg.variant {
        Variant::Bits => 0,
        Variant::Runs => 1,
    });
    out.push(tpl.len() as u8);
    crate::put_varint(&mut out, w as u64);
    crate::put_varint(&mut out, h as u64);
    crate::put_varint(&mut out, hs as u64);
    write_model(&mut out, &md);
    for s in &stripes {
        crate::put_varint(&mut out, s.len() as u64);
        out.extend_from_slice(s);
    }
    out
}

/// Lab statistics of one config on one plane: coded symbols split into
/// pixels outside runs, run tokens, and pixels skipped inside runs.
pub fn counts(plane: &[u8], w: usize, h: usize, cfg: &Cfg) -> (u64, u64, u64) {
    let tpl = template_of(cfg);
    let hs = cfg
        .stripe_rows
        .map(|v| (v as usize).max(1))
        .unwrap_or_else(|| crate::v1::default_stripe_rows(w as u32) as usize);
    let lw = w.div_ceil(NL);
    let widths = crate::v1::lane_widths(w, lw);
    let (mut syms, mut runs, mut skipped) = (0u64, 0u64, 0u64);
    for y0 in (0..h).step_by(hs) {
        let rows = (h - y0).min(hs);
        for l in 0..NL {
            lane_walk(
                plane,
                w,
                y0,
                rows,
                lw,
                &widths,
                tpl,
                cfg,
                l,
                &mut |_step, _ctx, sym| {
                    syms += 1;
                    if sym >= 2 {
                        runs += 1;
                        skipped += (sym - 1) as u64;
                    }
                },
            );
        }
    }
    (syms, runs, skipped)
}

/// `(width, height, file bytes, table bytes, stripe-stream bytes)`.
pub fn size_info(file: &[u8]) -> Result<(usize, usize, usize, usize, usize), Error> {
    let mut r = Rd { b: file, p: 0 };
    let hd = parse_head(&mut r)?;
    let tb = r.varint()? as usize;
    let table = r.bytes(tb)?;
    let _ = read_model(table, hd.variant, hd.tpl_len)?;
    let mut streams = 0usize;
    for _ in 0..hd.h.div_ceil(hd.hs) {
        let n = r.varint()? as usize;
        let _ = r.bytes(n)?;
        streams += n + crate::varint_len(n as u64);
    }
    Ok((
        hd.w,
        hd.h,
        file.len(),
        tb + crate::varint_len(tb as u64),
        streams,
    ))
}

// ---- scalar decoder --------------------------------------------------------

/// One row of one stripe; the reference decoder. `plane` is the full
/// step-major stripe plane (`rows * lw * NL`), rows above `row` decoded.
#[allow(clippy::too_many_arguments)]
fn decode_row_ref(
    md: &Model,
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    row: usize,
    w: usize,
    tpl: &[(i8, i8)],
    mixed: bool,
    plane: &mut [u16],
) {
    let base = row * lw * NL;
    let mut x = 0usize;
    while x < lw {
        // All-busy fast path (runs): every lane past its width or inside a
        // run, so the whole row segment can be filled white. Its purpose is
        // the decode-speed path runs exist for; it never fires for Bits.
        if mixed {
            let mut k = usize::MAX;
            let mut all = true;
            for l in 0..NL {
                if x as u32 >= widths[l] {
                    continue;
                }
                if st.run[l] > 0 {
                    k = k.min(st.run[l] as usize);
                } else {
                    all = false;
                    break;
                }
            }
            if all {
                let kk = k.min(lw - x);
                if kk > 0 {
                    for xx in x..x + kk {
                        plane[base + xx * NL..base + (xx + 1) * NL].fill(0);
                    }
                    for l in 0..NL {
                        let w = widths[l] as usize;
                        if x >= w {
                            continue;
                        }
                        if st.run[l] > 0 {
                            // A short lane is past its width before `kk`
                            // steps run out; only active positions count.
                            st.run[l] -= kk.min(w - x) as u32;
                        }
                    }
                    x += kk;
                    continue;
                }
            }
        }
        for l in 0..NL {
            let idx = base + x * NL + l;
            if x as u32 >= widths[l] {
                plane[idx] = 0;
                continue;
            }
            if st.run[l] > 0 {
                st.run[l] -= 1;
                plane[idx] = 0;
                continue;
            }
            let ctx = ctx_dec(plane, w, lw, row, x, l, tpl);
            let xs = st.x[l];
            if mixed && ctx == 0 {
                let slot = xs & (M - 1);
                let e = md.mixed.as_ref().unwrap().dec0[slot as usize];
                let mut nx = ((e & 1023) + 1) * (xs >> LOG_M) + ((e >> 10) & 1023);
                rans::renorm_one(&mut nx, &mut st.wp, words);
                st.x[l] = nx;
                let sym = (e >> 20) & 255;
                if sym >= 2 {
                    st.run[l] = sym - 2;
                    plane[idx] = 0;
                } else {
                    plane[idx] = sym as u16;
                }
            } else {
                let f0 = md.f0[ctx as usize] as u32;
                let slot = xs & (M - 1);
                let bit = slot >= f0;
                let f = if bit { M - f0 } else { f0 };
                let bias = if bit { slot - f0 } else { slot };
                let mut nx = f * (xs >> LOG_M) + bias;
                rans::renorm_one(&mut nx, &mut st.wp, words);
                st.x[l] = nx;
                plane[idx] = bit as u16;
            }
        }
        x += 1;
    }
}

// ---- SIMD row decoder (magetypes: v3 / neon / wasm128 / scalar) -----------

mod simd_row {
    use super::*;
    use archmage::prelude::*;

    /// Bulk steps (2 <= x && x + 2 < lw) assemble the 10 template bits from
    /// 16-lane loads; the four edge steps per row use the scalar context
    /// rule. The rANS step, renorm masks and ordered word pulls are the
    /// lossless coder's.
    #[magetypes(define(u16x16, u32x8), v3, neon, wasm128, scalar)]
    #[allow(clippy::too_many_arguments)]
    fn decode_impl<const MIXED: bool>(
        token: Token,
        md: &Model,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        w: usize,
        plane: &mut [u16],
    ) {
        let zero = u32x8::zero_t(token);
        let one = u32x8::splat_t(token, 1);
        let two = u32x8::splat_t(token, 2);
        let lim = u32x8::splat_t(token, rans::RANS_L);
        let slot_mask = u32x8::splat_t(token, M - 1);
        let full = u32x8::splat_t(token, M);
        let zero16 = u16x16::zero_t(token);
        let wv = [
            u32x8::from_slice_t(token, &widths[..8]),
            u32x8::from_slice_t(token, &widths[8..]),
        ];
        let mut run = [
            u32x8::from_slice_t(token, &st.run[..8]),
            u32x8::from_slice_t(token, &st.run[8..]),
        ];
        let mut wp = st.wp;
        let base_cur = row * lw * NL;
        let mut x = 0usize;
        while x < lw {
            let xv = u32x8::splat_t(token, x as u32);
            let inw = [wv[0].simd_gt(xv), wv[1].simd_gt(xv)];
            if MIXED {
                let busy0 = inw[0].not() | run[0].simd_gt(zero);
                let busy1 = inw[1].not() | run[1].simd_gt(zero);
                if busy0.bitmask() == 0xFF && busy1.bitmask() == 0xFF {
                    let big = u32x8::splat_t(token, lw as u32);
                    let r0 = u32x8::blend(inw[0], run[0], big).to_array();
                    let r1 = u32x8::blend(inw[1], run[1], big).to_array();
                    let kk =
                        (r0.iter().chain(r1.iter()).copied().min().unwrap() as usize).min(lw - x);
                    if kk > 0 {
                        for xx in x..x + kk {
                            plane[base_cur + xx * NL..base_cur + (xx + 1) * NL].fill(0);
                        }
                        let kv = u32x8::splat_t(token, kk as u32);
                        // A short lane is past its width before `kk` steps
                        // run out; only active positions count.
                        let dec0 = kv.min(wv[0] - xv);
                        let dec1 = kv.min(wv[1] - xv);
                        run[0] = run[0] - (run[0].simd_gt(zero) & inw[0] & dec0);
                        run[1] = run[1] - (run[1].simd_gt(zero) & inw[1] & dec1);
                        x += kk;
                        continue;
                    }
                }
            }
            let ctx16 = if x >= 2 && x + 2 < lw {
                let mut c = u16x16::from_slice_t(token, &plane[base_cur + (x - 2) * NL..])
                    .shl_const::<8>()
                    | u16x16::from_slice_t(token, &plane[base_cur + (x - 1) * NL..])
                        .shl_const::<9>();
                if row >= 2 {
                    let b = (row - 2) * lw * NL;
                    c |= u16x16::from_slice_t(token, &plane[b + (x - 1) * NL..])
                        | u16x16::from_slice_t(token, &plane[b + x * NL..]).shl_const::<1>()
                        | u16x16::from_slice_t(token, &plane[b + (x + 1) * NL..]).shl_const::<2>();
                }
                if row >= 1 {
                    let b = (row - 1) * lw * NL;
                    c |= u16x16::from_slice_t(token, &plane[b + (x - 2) * NL..]).shl_const::<3>()
                        | u16x16::from_slice_t(token, &plane[b + (x - 1) * NL..]).shl_const::<4>()
                        | u16x16::from_slice_t(token, &plane[b + x * NL..]).shl_const::<5>()
                        | u16x16::from_slice_t(token, &plane[b + (x + 1) * NL..]).shl_const::<6>()
                        | u16x16::from_slice_t(token, &plane[b + (x + 2) * NL..]).shl_const::<7>();
                }
                c
            } else {
                let mut a = [0u16; NL];
                for l in 0..NL {
                    a[l] = ctx_dec(plane, w, lw, row, x, l, &T10);
                }
                u16x16::from_array_t(token, a)
            };
            let ctx = [ctx16.widen_low(), ctx16.widen_high()];
            for h in 0..2 {
                let ch = ctx[h];
                let ctxa = ch.to_array();
                let xs = u32x8::from_slice_t(token, &st.x[8 * h..8 * h + 8]);
                let slot = xs & slot_mask;
                let slota = slot.to_array();
                let f0a: [u32; 8] = core::array::from_fn(|i| md.f0[ctxa[i] as usize] as u32);
                let f0v = u32x8::from_array_t(token, f0a);
                let bit = slot.simd_ge(f0v);
                let mut f = u32x8::blend(bit, full - f0v, f0v);
                let mut bias = slot - (bit & f0v);
                let mut symv = bit & one;
                if MIXED {
                    let e0a: [u32; 8] = core::array::from_fn(|i| {
                        if ctxa[i] == 0 {
                            md.mixed.as_ref().unwrap().dec0[(slota[i] & (M - 1)) as usize]
                        } else {
                            0
                        }
                    });
                    let e0 = u32x8::from_array_t(token, e0a);
                    let is0 = ch.simd_eq(zero);
                    f = u32x8::blend(is0, (e0 & slot_mask) + one, f);
                    bias = u32x8::blend(is0, (e0.shr_logical_const::<10>()) & slot_mask, bias);
                    symv = u32x8::blend(is0, e0.shr_logical_const::<20>(), symv);
                }
                let nx = f * xs.shr_logical_const::<10>() + bias;
                let in_run = run[h].simd_gt(zero) & inw[h];
                run[h] = run[h] - (in_run & one);
                let act = inw[h] & in_run.not();
                let xx = u32x8::blend(act, nx, xs);
                xx.store((&mut st.x[8 * h..8 * h + 8]).try_into().unwrap());
                let mut m = (act & xx.simd_lt(lim)).bitmask();
                while m != 0 {
                    let i = m.trailing_zeros() as usize;
                    st.x[8 * h + i] = (st.x[8 * h + i] << 16) | words.word(wp) as u32;
                    wp += 1;
                    m &= m - 1;
                }
                let mut val = act & symv;
                if MIXED {
                    let rn = act & symv.simd_ge(two);
                    run[h] = u32x8::blend(rn, symv - two, run[h]);
                    val = u32x8::blend(rn, zero, val);
                }
                let va = val.to_array();
                for i in 0..8 {
                    plane[base_cur + x * NL + 8 * h + i] = va[i] as u16;
                }
            }
            x += 1;
        }
        st.wp = wp;
        st.run[..8].copy_from_slice(&run[0].to_array());
        st.run[8..].copy_from_slice(&run[1].to_array());
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode_row(
        md: &Model,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        row: usize,
        w: usize,
        plane: &mut [u16],
        mixed: bool,
    ) {
        if mixed {
            incant!(
                decode_impl::<true>(md, words, st, widths, lw, row, w, plane),
                [v3, neon, wasm128, scalar]
            );
        } else {
            incant!(
                decode_impl::<false>(md, words, st, widths, lw, row, w, plane),
                [v3, neon, wasm128, scalar]
            );
        }
    }
}

// ---- incremental-context decoders (2026-10-11 lowbit lane) ----------------
//
// The T10 bit order keeps each template row contiguous with dx ascending, so
// moving one step right shifts the context down one bit, drops the leftmost
// pixel of each row and inserts three new pixels: row -2 at dx +1, row -1 at
// dx +2 and the pixel just decoded (row 0, dx -1). Rows are step-major u8
// planes of (lw + 2) * NL entries: the two extra steps hold, per lane, the
// columns just right of its segment (the next lane's first columns), so the
// inner loop reads every template pixel with one indexed load. Current-row
// pixels left of a lane's segment read the row above (0 on stripe row 0,
// where that row is all zeros), exactly the prototype's rule.

/// Bits 0, 1, 3-6 and 8 survive the shift; 2, 7 and 9 take new pixels.
const SLIDE: u32 = 0x17B;

/// Pixel of a step-major row at group column `col` (0 outside `0..w`).
#[inline(always)]
fn at_col(buf: &[u8], w: usize, lw: usize, col: isize) -> u32 {
    if col < 0 || col as usize >= w {
        return 0;
    }
    let c = col as usize;
    buf[(c % lw) * NL + c / lw] as u32
}

/// Context of lane `l` at step 0 (`p1`/`p2`: rows -1 and -2).
#[inline(always)]
fn ctx0(p2: &[u8], p1: &[u8], w: usize, lw: usize, l: usize) -> u32 {
    let c = (l * lw) as isize;
    let a = |b: &[u8], dx: isize| at_col(b, w, lw, c + dx);
    a(p2, -1)
        | a(p2, 0) << 1
        | a(p2, 1) << 2
        | a(p1, -2) << 3
        | a(p1, -1) << 4
        | a(p1, 0) << 5
        | a(p1, 1) << 6
        | a(p1, 2) << 7
        | a(p1, -2) << 8
        | a(p1, -1) << 9
}

/// Fills the two steps after a row's last with the next lanes' columns.
fn wrap_row(cur: &mut [u8], w: usize, lw: usize) {
    for l in 0..NL {
        for k in 0..2 {
            cur[(lw + k) * NL + l] = at_col(cur, w, lw, (l * lw + lw + k) as isize) as u8;
        }
    }
}

/// One row, 16 lanes interleaved in plain scalar code (one body for every
/// tier: the lanes' dependency chains overlap out of order).
#[allow(clippy::too_many_arguments)]
fn decode_row_fast(
    f0: &[u32; 1024],
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    w: usize,
    p2: &[u8],
    p1: &[u8],
    cur: &mut [u8],
) {
    let mut ctx = [0u32; NL];
    for (l, c) in ctx.iter_mut().enumerate() {
        *c = ctx0(p2, p1, w, lw, l);
    }
    let mut wp = st.wp;
    for x in 0..lw {
        for l in 0..NL {
            if x >= widths[l] as usize {
                cur[x * NL + l] = 0;
                continue;
            }
            let c = if x == 0 {
                ctx[l]
            } else {
                ((ctx[l] >> 1) & SLIDE)
                    | (p2[(x + 1) * NL + l] as u32) << 2
                    | (p1[(x + 2) * NL + l] as u32) << 7
                    | (cur[(x - 1) * NL + l] as u32) << 9
            };
            ctx[l] = c;
            let p = f0[c as usize & 1023];
            let s = st.x[l];
            let slot = s & (M - 1);
            let bit = (slot >= p) as u32;
            let m = 0u32.wrapping_sub(bit);
            let f = p ^ ((p ^ (M - p)) & m);
            let mut n = f * (s >> LOG_M) + slot - (p & m);
            if n < rans::RANS_L {
                n = (n << 16) | words.word(wp) as u32;
                wp += 1;
            }
            st.x[l] = n;
            cur[x * NL + l] = bit as u8;
        }
    }
    st.wp = wp;
    wrap_row(cur, w, lw);
}

#[cfg(target_arch = "x86_64")]
mod v4row {
    use super::*;
    use archmage::prelude::*;
    use magetypes::simd::generic::u32x16;

    /// The same row with the 16 lanes in one AVX-512 register and the
    /// probability lookup as one bounds-safe gather.
    #[arcane]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode_row_v4(
        token: X64V4Token,
        f0: &[u32; 1024],
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        w: usize,
        p2: &[u8],
        p1: &[u8],
        cur: &mut [u8],
    ) {
        let mut c0 = [0u32; NL];
        for (l, c) in c0.iter_mut().enumerate() {
            *c = ctx0(p2, p1, w, lw, l);
        }
        let mut ctx = u32x16::from_array_t(token, c0);
        let mut xs = u32x16::from_array_t(token, st.x);
        let wv = u32x16::from_array_t(token, *widths);
        let slide = u32x16::splat_t(token, SLIDE);
        let mm = u32x16::splat_t(token, M - 1);
        let full = u32x16::splat_t(token, M);
        let one = u32x16::splat_t(token, 1);
        let lim = u32x16::splat_t(token, rans::RANS_L);
        let mut prev = u32x16::zero_t(token);
        let mut wp = st.wp;
        let wid = |b: &[u8], i: usize| -> u32x16<X64V4Token> {
            let a: [u8; 16] = b[i..i + 16].try_into().unwrap();
            u32x16::from_array_t(token, a.map(|v| v as u32))
        };
        for x in 0..lw {
            if x > 0 {
                ctx = (ctx.shr_logical_const::<1>() & slide)
                    | wid(p2, (x + 1) * NL).shl_const::<2>()
                    | wid(p1, (x + 2) * NL).shl_const::<7>()
                    | prev.shl_const::<9>();
            }
            let p = u32x16::gather_wrapping(f0, ctx);
            let slot = xs & mm;
            let bit = slot.simd_ge(p);
            let f = u32x16::blend(bit, full - p, p);
            let n = f * xs.shr_logical_const::<10>() + slot - (bit & p);
            let xv = u32x16::splat_t(token, x as u32);
            let act = wv.simd_gt(xv);
            xs = u32x16::blend(act, n, xs);
            let mut m = (act & xs.simd_lt(lim)).bitmask();
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
            prev = act & bit & one;
            let pa = prev.to_array();
            let o: &mut [u8; 16] = (&mut cur[x * NL..x * NL + 16]).try_into().unwrap();
            *o = pa.map(|v| v as u8);
        }
        st.x = xs.to_array();
        st.wp = wp;
        wrap_row(cur, w, lw);
    }
}

/// One portable magetypes body for every tier (u32 rows, 16 lanes as one
/// `u32x16`): the probability lookup is a gather on AVX-512 and 16 lane
/// loads elsewhere ([`port::Probs`]).
mod port {
    use super::*;
    use archmage::prelude::*;
    use magetypes::simd::backends::U32x16Backend;
    use magetypes::simd::generic::u32x16;

    /// 16 probabilities at 16 contexts.
    pub(super) trait Probs: U32x16Backend + Copy {
        #[inline(always)]
        fn probs(self, t: &[u32; 1024], c: u32x16<Self>) -> u32x16<Self> {
            u32x16::from_array_t(self, c.to_array().map(|i| t[i as usize & 1023]))
        }
    }
    #[cfg(target_arch = "x86_64")]
    impl Probs for X64V4Token {
        #[inline(always)]
        fn probs(self, t: &[u32; 1024], c: u32x16<Self>) -> u32x16<Self> {
            u32x16::gather_wrapping(t, c)
        }
    }
    #[cfg(target_arch = "x86_64")]
    impl Probs for X64V3Token {}
    #[cfg(target_arch = "aarch64")]
    impl Probs for NeonToken {}
    #[cfg(target_arch = "wasm32")]
    impl Probs for Wasm128Token {}
    impl Probs for ScalarToken {}

    #[magetypes(define(u32x16), v4, v3, neon, wasm128, scalar)]
    #[allow(clippy::too_many_arguments)]
    fn row_impl(
        token: Token,
        f0: &[u32; 1024],
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        w: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
    ) {
        let mut c0 = [0u32; NL];
        for (l, c) in c0.iter_mut().enumerate() {
            *c = ctx0_32(p2, p1, w, lw, l);
        }
        let mut ctx = u32x16::from_array_t(token, c0);
        let mut xs = u32x16::from_array_t(token, st.x);
        let wv = u32x16::from_array_t(token, *widths);
        let slide = u32x16::splat_t(token, SLIDE);
        let mm = u32x16::splat_t(token, M - 1);
        let full = u32x16::splat_t(token, M);
        let one = u32x16::splat_t(token, 1);
        let lim = u32x16::splat_t(token, rans::RANS_L);
        let mut prev = u32x16::zero_t(token);
        let mut wp = st.wp;
        for x in 0..lw {
            if x > 0 {
                ctx = (ctx.shr_logical_const::<1>() & slide)
                    | u32x16::from_slice_t(token, &p2[(x + 1) * NL..]).shl_const::<2>()
                    | u32x16::from_slice_t(token, &p1[(x + 2) * NL..]).shl_const::<7>()
                    | prev.shl_const::<9>();
            }
            let p = token.probs(f0, ctx);
            let slot = xs & mm;
            let bit = slot.simd_ge(p);
            let f = u32x16::blend(bit, full - p, p);
            let n = f * xs.shr_logical_const::<10>() + slot - (bit & p);
            let act = wv.simd_gt(u32x16::splat_t(token, x as u32));
            xs = u32x16::blend(act, n, xs);
            let mut m = (act & xs.simd_lt(lim)).bitmask();
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
            prev = act & bit & one;
            prev.store((&mut cur[x * NL..x * NL + 16]).try_into().unwrap());
        }
        st.x = xs.to_array();
        st.wp = wp;
    }

    /// `row_impl` with the lookup one step ahead: a 512-entry table packs the
    /// probabilities of both values of the context's newest bit (`f0[b]` low,
    /// `f0[b | 512]` high), so step x + 1's lookup needs only bits known when
    /// step x starts and leaves the bit-to-bit dependency chain.
    #[magetypes(define(u32x16), v4, v3, neon, wasm128, scalar)]
    #[allow(clippy::too_many_arguments)]
    fn row_ahead(
        token: Token,
        f2: &[u32; 512],
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        w: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
    ) {
        let mut c0 = [0u32; NL];
        for (l, c) in c0.iter_mut().enumerate() {
            *c = ctx0_32(p2, p1, w, lw, l);
        }
        let c0v = u32x16::from_array_t(token, c0);
        let low9 = u32x16::splat_t(token, 511);
        let mut base = c0v & low9;
        let mut prev = c0v.shr_logical_const::<9>();
        let look =
            |b: u32x16| u32x16::from_array_t(token, b.to_array().map(|i| f2[i as usize & 511]));
        let mut pp = look(base);
        let mut xs = u32x16::from_array_t(token, st.x);
        let wv = u32x16::from_array_t(token, *widths);
        let slide = u32x16::splat_t(token, SLIDE);
        let mm = u32x16::splat_t(token, M - 1);
        let lo16 = u32x16::splat_t(token, 0xFFFF);
        let full = u32x16::splat_t(token, M);
        let one = u32x16::splat_t(token, 1);
        let lim = u32x16::splat_t(token, rans::RANS_L);
        let mut wp = st.wp;
        for x in 0..lw {
            let pbit = prev.simd_ne(u32x16::zero_t(token));
            let p = u32x16::blend(pbit, pp.shr_logical_const::<16>(), pp & lo16);
            // Step x + 1: everything but this step's bit is known now.
            base = ((base | prev.shl_const::<9>()).shr_logical_const::<1>() & slide)
                | u32x16::from_slice_t(token, &p2[(x + 2) * NL..]).shl_const::<2>()
                | u32x16::from_slice_t(token, &p1[(x + 3) * NL..]).shl_const::<7>();
            pp = look(base);
            let slot = xs & mm;
            let bit = slot.simd_ge(p);
            let f = u32x16::blend(bit, full - p, p);
            let n = f * xs.shr_logical_const::<10>() + slot - (bit & p);
            let act = wv.simd_gt(u32x16::splat_t(token, x as u32));
            xs = u32x16::blend(act, n, xs);
            let mut m = (act & xs.simd_lt(lim)).bitmask();
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
            prev = act & bit & one;
            prev.store((&mut cur[x * NL..x * NL + 16]).try_into().unwrap());
        }
        st.x = xs.to_array();
        st.wp = wp;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode_row_ahead(
        f2: &[u32; 512],
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        w: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
        tier: &str,
    ) {
        #[cfg(target_arch = "x86_64")]
        {
            use archmage::SimdToken;
            #[cfg(feature = "avx512")]
            if tier == "ahead4"
                && let Some(t) = X64V4Token::summon()
            {
                return row_ahead_v4(t, f2, words, st, widths, lw, w, p2, p1, cur);
            }
            if tier != "aheadS"
                && let Some(t) = X64V3Token::summon()
            {
                return row_ahead_v3(t, f2, words, st, widths, lw, w, p2, p1, cur);
            }
            return row_ahead_scalar(ScalarToken, f2, words, st, widths, lw, w, p2, p1, cur);
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            if tier == "aheadS" {
                return row_ahead_scalar(ScalarToken, f2, words, st, widths, lw, w, p2, p1, cur);
            }
            incant!(
                row_ahead(f2, words, st, widths, lw, w, p2, p1, cur),
                [neon, wasm128, scalar]
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn decode_row(
        f0: &[u32; 1024],
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        w: usize,
        p2: &[u32],
        p1: &[u32],
        cur: &mut [u32],
        tier: &str,
    ) {
        #[cfg(target_arch = "x86_64")]
        {
            use archmage::SimdToken;
            #[cfg(feature = "avx512")]
            if tier != "port3"
                && let Some(t) = X64V4Token::summon()
            {
                return row_impl_v4(t, f0, words, st, widths, lw, w, p2, p1, cur);
            }
            if tier != "portS"
                && let Some(t) = X64V3Token::summon()
            {
                return row_impl_v3(t, f0, words, st, widths, lw, w, p2, p1, cur);
            }
            return row_impl_scalar(ScalarToken, f0, words, st, widths, lw, w, p2, p1, cur);
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            if tier == "portS" {
                return row_impl_scalar(ScalarToken, f0, words, st, widths, lw, w, p2, p1, cur);
            }
            incant!(
                row_impl(f0, words, st, widths, lw, w, p2, p1, cur),
                [neon, wasm128, scalar]
            );
        }
    }
}

#[inline(always)]
fn at_col32(buf: &[u32], w: usize, lw: usize, col: isize) -> u32 {
    if col < 0 || col as usize >= w {
        return 0;
    }
    let c = col as usize;
    buf[(c % lw) * NL + c / lw]
}

#[inline(always)]
fn ctx0_32(p2: &[u32], p1: &[u32], w: usize, lw: usize, l: usize) -> u32 {
    let c = (l * lw) as isize;
    let a = |b: &[u32], dx: isize| at_col32(b, w, lw, c + dx);
    a(p2, -1)
        | a(p2, 0) << 1
        | a(p2, 1) << 2
        | a(p1, -2) << 3
        | a(p1, -1) << 4
        | a(p1, 0) << 5
        | a(p1, 1) << 6
        | a(p1, 2) << 7
        | a(p1, -2) << 8
        | a(p1, -1) << 9
}

#[allow(clippy::too_many_arguments)]
fn decode_stripe_port(
    md: &Model,
    w: usize,
    rows: usize,
    lw: usize,
    widths: &[u32; NL],
    bytes: &[u8],
    out: &mut [u8],
    tier: &str,
) -> Result<(), Error> {
    let active = crate::v1::active_lanes(widths);
    let (words, x) = Words::from_bytes(bytes, active)?;
    let mut st = LaneState {
        x,
        run: [0; NL],
        wp: 0,
    };
    let mut f0 = [0u32; 1024];
    for (d, &s) in f0.iter_mut().zip(&md.f0) {
        *d = s as u32;
    }
    let mut f2 = [0u32; 512];
    for (b, d) in f2.iter_mut().enumerate() {
        *d = f0[b] | f0[b | 512] << 16;
    }
    let n = (lw + 3) * NL;
    let mut b = [vec![0u32; n], vec![0u32; n], vec![0u32; n]];
    for row in 0..rows {
        let [b0, b1, b2] = &mut b;
        let (p2, p1, cur) = match row % 3 {
            0 => (&*b1, &*b2, b0),
            1 => (&*b2, &*b0, b1),
            _ => (&*b0, &*b1, b2),
        };
        if tier.starts_with("ahead") {
            port::decode_row_ahead(&f2, &words, &mut st, widths, lw, w, p2, p1, cur, tier);
        } else {
            port::decode_row(&f0, &words, &mut st, widths, lw, w, p2, p1, cur, tier);
        }
        for l in 0..NL {
            for k in 0..2 {
                cur[(lw + k) * NL + l] = at_col32(cur, w, lw, (l * lw + lw + k) as isize);
            }
        }
        let o = &mut out[row * w..row * w + w];
        for l in 0..NL {
            for xx in 0..widths[l] as usize {
                o[l * lw + xx] = cur[xx * NL + l] as u8;
            }
        }
    }
    if st.wp > words.len {
        return Err(Error::Corrupt("lane stream overrun"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_stripe_fast(
    md: &Model,
    w: usize,
    rows: usize,
    lw: usize,
    widths: &[u32; NL],
    bytes: &[u8],
    out: &mut [u8],
    v4: bool,
) -> Result<(), Error> {
    let active = crate::v1::active_lanes(widths);
    let (words, x) = Words::from_bytes(bytes, active)?;
    let mut st = LaneState {
        x,
        run: [0; NL],
        wp: 0,
    };
    let mut f0 = [0u32; 1024];
    for (d, &s) in f0.iter_mut().zip(&md.f0) {
        *d = s as u32;
    }
    let n = (lw + 2) * NL;
    let mut b = [vec![0u8; n], vec![0u8; n], vec![0u8; n]];
    #[cfg(target_arch = "x86_64")]
    let tok = if v4 {
        <archmage::X64V4Token as archmage::SimdToken>::summon()
    } else {
        None
    };
    for row in 0..rows {
        let [b0, b1, b2] = &mut b;
        let (p2, p1, cur) = match row % 3 {
            0 => (&*b1, &*b2, b0),
            1 => (&*b2, &*b0, b1),
            _ => (&*b0, &*b1, b2),
        };
        #[cfg(target_arch = "x86_64")]
        if let Some(t) = tok {
            v4row::decode_row_v4(t, &f0, &words, &mut st, widths, lw, w, p2, p1, cur);
        } else {
            decode_row_fast(&f0, &words, &mut st, widths, lw, w, p2, p1, cur);
        }
        #[cfg(not(target_arch = "x86_64"))]
        decode_row_fast(&f0, &words, &mut st, widths, lw, w, p2, p1, cur);
        let o = &mut out[row * w..row * w + w];
        for l in 0..NL {
            for xx in 0..widths[l] as usize {
                o[l * lw + xx] = cur[xx * NL + l];
            }
        }
    }
    if st.wp > words.len {
        return Err(Error::Corrupt("lane stream overrun"));
    }
    Ok(())
}

/// [`decode`] with a named decoder: `ref`, `simd` (the prototype's
/// magetypes kernel), `fast` (incremental contexts, scalar) or `v4`
/// (incremental contexts, AVX-512 gather; `fast` elsewhere). 10-pixel
/// template, `Bits` variant only for `fast` and `v4`.
pub fn decode_tier(
    file: &[u8],
    out: &mut [u8],
    threads: usize,
    tier: &str,
) -> Result<(usize, usize), Error> {
    if tier == "ref" || tier == "simd" {
        return decode(file, out, threads, tier == "simd");
    }
    let mut r = Rd { b: file, p: 0 };
    let hd = parse_head(&mut r)?;
    if hd.variant != Variant::Bits || hd.tpl_len != 10 {
        return Err(Error::InvalidInput("fast decoders take config a only"));
    }
    let tb = r.varint()? as usize;
    let table = r.bytes(tb)?;
    let md = read_model(table, hd.variant, hd.tpl_len)?;
    let lw = hd.w.div_ceil(NL);
    let widths = crate::v1::lane_widths(hd.w, lw);
    let nstripes = hd.h.div_ceil(hd.hs);
    let mut stripes: Vec<&[u8]> = Vec::with_capacity(nstripes);
    for _ in 0..nstripes {
        let n = r.varint()? as usize;
        stripes.push(r.bytes(n)?);
    }
    if out.len() < hd.w * hd.h {
        return Err(Error::InvalidInput("bilevel output buffer too small"));
    }
    let out = &mut out[..hd.w * hd.h];
    let items: Vec<(&[u8], &mut [u8])> = stripes
        .into_iter()
        .zip(out.chunks_mut(hd.hs * hd.w))
        .collect();
    let v4 = tier == "v4";
    let results = crate::par::par_map(items, threads, |_, (bytes, chunk)| {
        let rows = chunk.len() / hd.w;
        if tier.starts_with("port") || tier.starts_with("ahead") {
            decode_stripe_port(&md, hd.w, rows, lw, &widths, bytes, chunk, tier)
        } else {
            decode_stripe_fast(&md, hd.w, rows, lw, &widths, bytes, chunk, v4)
        }
    });
    for res in results {
        res?;
    }
    Ok((hd.w, hd.h))
}

#[allow(clippy::too_many_arguments)]
fn decode_stripe(
    md: &Model,
    w: usize,
    rows: usize,
    lw: usize,
    widths: &[u32; NL],
    tpl: &[(i8, i8)],
    mixed: bool,
    simd: bool,
    bytes: &[u8],
    out: &mut [u8],
) -> Result<(), Error> {
    let active = crate::v1::active_lanes(widths);
    let (words, x) = Words::from_bytes(bytes, active)?;
    let mut st = LaneState {
        x,
        run: [0; NL],
        wp: 0,
    };
    let mut plane = vec![0u16; rows * lw * NL + 64];
    for row in 0..rows {
        if simd && tpl.len() == 10 {
            simd_row::decode_row(md, &words, &mut st, widths, lw, row, w, &mut plane, mixed);
        } else {
            decode_row_ref(
                md, &words, &mut st, widths, lw, row, w, tpl, mixed, &mut plane,
            );
        }
    }
    for row in 0..rows {
        for l in 0..NL {
            let width = widths[l] as usize;
            for xx in 0..width {
                out[row * w + l * lw + xx] = plane[(row * lw + xx) * NL + l] as u8;
            }
        }
    }
    Ok(())
}

/// Decodes into `out` (`w * h` bytes, 1 = black). `threads` decodes
/// stripes in parallel; `simd` selects the magetypes row kernel for the
/// 10-pixel template (the 16-pixel template stays scalar). Returns
/// `(width, height)`.
pub fn decode(
    file: &[u8],
    out: &mut [u8],
    threads: usize,
    simd: bool,
) -> Result<(usize, usize), Error> {
    let mut r = Rd { b: file, p: 0 };
    let hd = parse_head(&mut r)?;
    let tb = r.varint()? as usize;
    let table = r.bytes(tb)?;
    let md = read_model(table, hd.variant, hd.tpl_len)?;
    let lw = hd.w.div_ceil(NL);
    let widths = crate::v1::lane_widths(hd.w, lw);
    let tpl: &[(i8, i8)] = if hd.tpl_len == 16 { &T16 } else { &T10 };
    let mixed = hd.variant == Variant::Runs;
    let nstripes = hd.h.div_ceil(hd.hs);
    let mut stripes: Vec<&[u8]> = Vec::with_capacity(nstripes);
    for _ in 0..nstripes {
        let n = r.varint()? as usize;
        stripes.push(r.bytes(n)?);
    }
    if out.len() < hd.w * hd.h {
        return Err(Error::InvalidInput("bilevel output buffer too small"));
    }
    let out = &mut out[..hd.w * hd.h];
    let items: Vec<(&[u8], &mut [u8])> = stripes
        .into_iter()
        .zip(out.chunks_mut(hd.hs * hd.w))
        .collect();
    let results = crate::par::par_map(items, threads, |_, (bytes, chunk)| {
        let rows = chunk.len() / hd.w;
        decode_stripe(&md, hd.w, rows, lw, &widths, tpl, mixed, simd, bytes, chunk)
    });
    for res in results {
        res?;
    }
    Ok((hd.w, hd.h))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn pattern(kind: usize, w: usize, h: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match kind {
                    0 => 0,
                    1 => 1,
                    2 => ((x + y) % 2) as u8,
                    3 => (rng(&mut s) & 1) as u8,
                    4 => ((x / 3 + y / 5) % 2) as u8,
                    5 => {
                        let (xc, yc) = (w / 2, h / 2);
                        let (dx, dy) = (x as i64 - xc as i64, y as i64 - yc as i64);
                        ((dx * dx + dy * dy) < ((w.min(h) as i64) * (w.min(h) as i64) / 16)) as u8
                    }
                    6 => (rng(&mut s) % 100 < 7) as u8,
                    _ => (y % 7 == 0) as u8,
                }
            })
            .collect()
    }

    fn roundtrip(w: usize, h: usize, cfg: &Cfg, simd: bool, kind: usize) {
        let p = pattern(kind, w, h, 0x1234_5678 ^ ((w * 131 + h) as u64));
        let file = encode(&p, w, h, cfg, 1);
        let mut back = vec![7u8; w * h];
        let (dw, dh) = decode(&file, &mut back, 1, simd).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, p, "cfg={cfg:?} simd={simd} kind={kind} {w}x{h}");
        let mut back2 = vec![7u8; w * h];
        decode(&file, &mut back2, 4, !simd).unwrap();
        assert_eq!(back2, p, "threads/parity {w}x{h}");
        if cfg.variant == Variant::Bits && !cfg.template16 {
            for tier in [
                "fast", "v4", "port", "port3", "portS", "ahead", "ahead4", "aheadS",
            ] {
                let mut back3 = vec![7u8; w * h];
                decode_tier(&file, &mut back3, 2, tier).unwrap();
                assert_eq!(back3, p, "{tier} kind={kind} {w}x{h}");
            }
        }
    }

    #[test]
    fn roundtrip_small_geometry() {
        for kind in 0..8 {
            for &(w, h) in &[
                (1, 1),
                (1, 9),
                (2, 3),
                (3, 5),
                (7, 7),
                (15, 6),
                (16, 4),
                (17, 9),
                (31, 3),
                (32, 33),
                (33, 2),
                (64, 5),
                (100, 12),
            ] {
                for &(variant, template16) in &[
                    (Variant::Bits, false),
                    (Variant::Runs, false),
                    (Variant::Bits, true),
                    (Variant::Runs, true),
                ] {
                    let cfg = Cfg {
                        variant,
                        template16,
                        run_min: 2,
                        stripe_rows: Some(3),
                    };
                    roundtrip(w, h, &cfg, false, kind);
                    if !template16 {
                        roundtrip(w, h, &cfg, true, kind);
                    }
                }
            }
        }
    }

    #[test]
    fn roundtrip_default_stripes_and_simd_edges() {
        for kind in 0..8 {
            for &(w, h) in &[(232, 64), (233, 65), (145, 64), (144, 3), (16, 1), (5, 1)] {
                let cfg = Cfg::default();
                roundtrip(w, h, &cfg, true, kind);
                let cfg = Cfg {
                    variant: Variant::Runs,
                    run_min: 4,
                    ..Cfg::default()
                };
                roundtrip(w, h, &cfg, true, kind);
                let cfg = Cfg {
                    template16: true,
                    ..Cfg::default()
                };
                roundtrip(w, h, &cfg, false, kind);
            }
        }
    }

    #[test]
    fn run_min_sweep_roundtrips() {
        for run_min in [2, 3, 4, 7, 200] {
            let cfg = Cfg {
                variant: Variant::Runs,
                run_min,
                ..Cfg::default()
            };
            roundtrip(129, 40, &cfg, true, 0);
            roundtrip(129, 40, &cfg, true, 3);
            roundtrip(129, 40, &cfg, true, 4);
        }
    }

    #[test]
    fn deterministic_bytes() {
        let p = pattern(3, 71, 23, 99);
        let cfg = Cfg {
            variant: Variant::Runs,
            ..Cfg::default()
        };
        let a = encode(&p, 71, 23, &cfg, 1);
        let b = encode(&p, 71, 23, &cfg, 4);
        assert_eq!(a, b);
    }

    #[test]
    fn long_runs_and_blank_pages() {
        for kind in [0usize, 1] {
            for &(w, h) in &[(64, 200), (2320, 64)] {
                let cfg = Cfg {
                    variant: Variant::Runs,
                    run_min: 2,
                    stripe_rows: Some(16),
                    ..Cfg::default()
                };
                roundtrip(w, h, &cfg, true, kind);
            }
        }
    }

    #[test]
    fn corrupt_files_rejected() {
        let p = pattern(4, 40, 20, 7);
        let file = encode(&p, 40, 20, &Cfg::default(), 1);
        let mut back = vec![0u8; 40 * 20];
        assert!(decode(&file[..5], &mut back, 1, false).is_err());
        assert!(decode(&file[..file.len() - 3], &mut back, 1, false).is_err());
        let mut bad = file.clone();
        bad[6] = 9;
        assert!(decode(&bad, &mut back, 1, false).is_err());
        let mut short = vec![0u8; 10];
        assert!(decode(&file, &mut short, 1, false).is_err());
    }

    #[test]
    fn size_info_adds_up() {
        let p = pattern(4, 100, 40, 3);
        let cfg = Cfg {
            variant: Variant::Runs,
            ..Cfg::default()
        };
        let file = encode(&p, 100, 40, &cfg, 1);
        let (w, h, total, table, streams) = size_info(&file).unwrap();
        assert_eq!((w, h), (100, 40));
        assert_eq!(total, file.len());
        assert!(table + streams <= total);
        assert!(total - table - streams < 16);
    }
}
