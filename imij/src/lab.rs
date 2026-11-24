#![allow(
    dead_code,
    unused_mut,
    unused_variables,
    private_interfaces,
    clippy::all
)]
//! Entropy-layer laboratory: encode/decode residual planes with candidate
//! coders under one token + context model so decode speed and size can be
//! compared in isolation. Not part of the format.

use alloc::vec;
use alloc::vec::Vec;

use crate::Analysis;
use crate::ans;
use crate::bits::{BitReader, BitWriter};
use crate::coding_layout::{CTX_CLASSES, ctx_class};
use crate::encode::choose_ctx_map;
use crate::huffman::{self, DecodeTable};
use crate::tokens::{TokenCfg, hybrid_base};

/// One stripe in lane-major layout.
pub struct LaneStripe {
    pub rows: usize,
    /// Width of each lane.
    pub widths: Vec<usize>,
    /// Per slot: lane-major residuals (lane 0 rows..., lane 1 rows...).
    pub u: Vec<Vec<u16>>,
}

impl LaneStripe {
    pub fn lane_offset(&self, l: usize) -> usize {
        self.widths[..l].iter().sum::<usize>() * self.rows
    }
    pub fn pixels(&self) -> usize {
        self.widths.iter().sum::<usize>() * self.rows
    }
}

pub fn lane_stripes(a: &Analysis) -> Vec<LaneStripe> {
    let mut out = Vec::new();
    let (w, h, hs, sw) = (a.w, a.h, a.hs, a.sw);
    for y0 in (0..h).step_by(hs) {
        let y1 = (y0 + hs).min(h);
        let rows = y1 - y0;
        let widths: Vec<usize> = (0..w).step_by(sw).map(|xs| (xs + sw).min(w) - xs).collect();
        let u = a
            .res
            .iter()
            .map(|r| {
                let mut v = Vec::with_capacity(rows * w);
                for xs in (0..w).step_by(sw) {
                    let xe = (xs + sw).min(w);
                    for y in y0..y1 {
                        v.extend(r[y * w + xs..y * w + xe].iter().map(|&u| u as u16));
                    }
                }
                v
            })
            .collect();
        out.push(LaneStripe { rows, widths, u });
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coder {
    Huffman { max_len: u32 },
    Tans { log_m: u32 },
}

#[derive(Clone, Copy, Debug)]
pub struct LabCfg {
    pub coder: Coder,
    pub k: u32,
    pub m: u32,
    pub run_min: u32,
    pub max_ctx: usize,
    pub cross: bool,
    pub color_slots: usize,
}

struct SlotModel {
    tc: TokenCfg,
    ctx_map: [u8; CTX_CLASSES],
    /// value-token (base, extra bits) for tokens >= 2^k; run-token (base, bits).
    vlut: Vec<(u32, u32)>,
    rlut: Vec<(u32, u32)>,
    huff: Vec<DecodeTable>,
    huff_enc: Vec<(Vec<u8>, Vec<u16>)>,
    tans: Vec<TansTable>,
}

pub struct LabEncoded {
    cfg: LabCfg,
    slots: Vec<SlotModel>,
    /// `[stripe][lane][slot]` stream bytes.
    pub streams: Vec<Vec<Vec<Vec<u8>>>>,
    pub payload_bytes: usize,
    pub tokens: u64,
}

#[derive(Clone, Copy)]
struct Tok {
    ctx: u8,
    tok: u16,
    nb: u8,
    extra: u32,
}

#[inline(always)]
fn ctx_sum_lane(u: &[u16], off: usize, lw: usize, j: usize) -> u32 {
    let x = j % lw;
    let uw = if x > 0 {
        Some(u[off + j - 1] as u32)
    } else {
        None
    };
    let un = if j >= lw {
        Some(u[off + j - lw] as u32)
    } else {
        None
    };
    match (uw, un) {
        (Some(a), Some(b)) => a + b,
        (Some(a), None) => 2 * a,
        (None, Some(b)) => 2 * b,
        (None, None) => 0,
    }
}

fn tokenize(
    st: &LaneStripe,
    s: usize,
    cfg: &LabCfg,
    tc: &TokenCfg,
    l: usize,
    map: Option<&[u8; CTX_CLASSES]>,
    out: &mut Vec<Tok>,
) {
    let u = &st.u[s];
    let off = st.lane_offset(l);
    let lw = st.widths[l];
    let n = lw * st.rows;
    let cross = cfg.cross && s >= 1 && s < cfg.color_slots;
    let mut j = 0;
    while j < n {
        let mut sum = ctx_sum_lane(u, off, lw, j);
        if cross {
            sum += st.u[0][off + j] as u32;
        }
        let class = ctx_class(sum);
        let ctx = map.map(|m| m[class]).unwrap_or(class as u8);
        let v = u[off + j] as u32;
        if v == 0 && cfg.run_min > 0 {
            let mut len = 1;
            while j + len < n && u[off + j + len] == 0 {
                len += 1;
            }
            if len as u32 >= cfg.run_min {
                let (t, e, nb) = tc.run(len as u32);
                out.push(Tok {
                    ctx,
                    tok: t as u16,
                    nb: nb as u8,
                    extra: e,
                });
                j += len;
                continue;
            }
        }
        let (t, e, nb) = tc.value(v);
        out.push(Tok {
            ctx,
            tok: t as u16,
            nb: nb as u8,
            extra: e,
        });
        j += 1;
    }
}

pub fn encode(stripes: &[LaneStripe], slot_bits: &[u32], cfg: LabCfg) -> LabEncoded {
    let ns = slot_bits.len();
    let mut slots = Vec::with_capacity(ns);
    for s in 0..ns {
        let tc = TokenCfg::new(cfg.k, cfg.m, slot_bits[s], 4, 1);
        let asz = tc.alphabet() as usize;
        let mut hist = vec![vec![0u64; asz]; CTX_CLASSES];
        let mut toks = Vec::new();
        for st in stripes {
            for l in 0..st.widths.len() {
                toks.clear();
                tokenize(st, s, &cfg, &tc, l, None, &mut toks);
                for t in &toks {
                    hist[t.ctx as usize][t.tok as usize] += 1;
                }
            }
        }
        let (ctx_map, groups) = choose_ctx_map(&hist, cfg.max_ctx, 12);
        let vt = tc.value_tokens();
        let vlut = (0..vt).map(|t| hybrid_base(t, tc.k, tc.m)).collect();
        let rlut = (0..tc.run_tokens())
            .map(|t| hybrid_base(t, tc.rk, tc.rm))
            .collect();
        let mut sm = SlotModel {
            tc,
            ctx_map,
            vlut,
            rlut,
            huff: Vec::new(),
            huff_enc: Vec::new(),
            tans: Vec::new(),
        };
        for g in &groups {
            match cfg.coder {
                Coder::Huffman { max_len } => {
                    let lens = huffman::build_lengths(g, max_len);
                    let used = g.iter().filter(|&&c| c > 0).count();
                    sm.huff.push(if used <= 1 {
                        DecodeTable::single(g.iter().position(|&c| c > 0).unwrap_or(0) as u16)
                    } else {
                        DecodeTable::build(&lens).unwrap()
                    });
                    let codes = huffman::canonical_codes(&lens);
                    sm.huff_enc.push((lens, codes));
                }
                Coder::Tans { log_m } => {
                    let mut q = ans::normalize(g, log_m).expect("too many symbols for tANS table");
                    if q.iter().all(|&v| v == 0) {
                        q[0] = 1 << log_m;
                    }
                    sm.tans.push(TansTable::new(&q, log_m));
                }
            }
        }
        slots.push(sm);
    }
    // Streams.
    let mut streams = Vec::with_capacity(stripes.len());
    let mut payload = 0usize;
    let mut tokens = 0u64;
    let mut toks = Vec::new();
    for st in stripes {
        let mut lanes = Vec::with_capacity(st.widths.len());
        for l in 0..st.widths.len() {
            let mut per = Vec::with_capacity(ns);
            for s in 0..ns {
                toks.clear();
                let sm = &slots[s];
                tokenize(st, s, &cfg, &sm.tc, l, Some(&sm.ctx_map), &mut toks);
                tokens += toks.len() as u64;
                let bytes = match cfg.coder {
                    Coder::Huffman { .. } => {
                        let mut bw = BitWriter::new();
                        for t in &toks {
                            let (lens, codes) = &sm.huff_enc[t.ctx as usize];
                            bw.put(codes[t.tok as usize] as u32, lens[t.tok as usize] as u32);
                            if t.nb > 0 {
                                bw.put(t.extra, t.nb as u32);
                            }
                        }
                        bw.finish()
                    }
                    Coder::Tans { log_m } => tans_encode(&toks, &sm.tans, log_m),
                };
                payload += bytes.len();
                per.push(bytes);
            }
            lanes.push(per);
        }
        streams.push(lanes);
    }
    LabEncoded {
        cfg,
        slots,
        streams,
        payload_bytes: payload,
        tokens,
    }
}

impl LabEncoded {
    pub fn is_tans(&self) -> bool {
        matches!(self.cfg.coder, Coder::Tans { .. })
    }
}

// ---- tANS -----------------------------------------------------------------

pub(crate) struct TansTable {
    /// Decode entry: sym | nb << 8 | base << 16.
    dec: Vec<u32>,
    /// Encode: per symbol, freq and the state positions in spread order.
    freq: Vec<u32>,
    pos: Vec<Vec<u16>>,
}

impl TansTable {
    fn new(q: &[u32], log_m: u32) -> Self {
        let m = 1usize << log_m;
        let mut spread = vec![0u16; m];
        let step = (m >> 1) + (m >> 3) + 3;
        let mask = m - 1;
        let mut p = 0usize;
        for (s, &f) in q.iter().enumerate() {
            for _ in 0..f {
                spread[p] = s as u16;
                p = (p + step) & mask;
            }
        }
        let mut next: Vec<u32> = q.to_vec();
        let mut pos = vec![Vec::new(); q.len()];
        let mut dec = vec![0u32; m];
        for (x, &s) in spread.iter().enumerate() {
            let n = next[s as usize];
            next[s as usize] += 1;
            let nb = log_m - (31 - n.leading_zeros());
            let base = (n << nb) - m as u32;
            dec[x] = s as u32 | (nb << 8) | (base << 16);
            pos[s as usize].push(x as u16);
        }
        Self {
            dec,
            freq: q.to_vec(),
            pos,
        }
    }
}

fn tans_encode(toks: &[Tok], tables: &[TansTable], log_m: u32) -> Vec<u8> {
    let m = 1u32 << log_m;
    // Reverse pass: compute per-symbol (bits, nb) and the initial state.
    let mut out_bits: Vec<(u32, u32)> = vec![(0, 0); toks.len()];
    let mut x = m; // encoder state in [M, 2M)
    for (i, t) in toks.iter().enumerate().rev() {
        let tb = &tables[t.ctx as usize];
        let f = tb.freq[t.tok as usize];
        let mut nb = 0;
        while (x >> nb) >= 2 * f {
            nb += 1;
        }
        let bits = x & ((1 << nb) - 1);
        let n = x >> nb;
        out_bits[i] = (bits, nb);
        x = m + tb.pos[t.tok as usize][(n - f) as usize] as u32;
    }
    let mut bw = BitWriter::new();
    bw.put(x - m, log_m);
    for (i, t) in toks.iter().enumerate() {
        let (b, nb) = out_bits[i];
        if nb > 0 {
            bw.put(b, nb);
        }
        if t.nb > 0 {
            bw.put(t.extra, t.nb as u32);
        }
    }
    bw.finish()
}

// ---- decoders ---------------------------------------------------------------

struct LaneState<'a> {
    r: BitReader<'a>,
    x: u32,
    run: u32,
}

#[inline(always)]
fn decode_one(sm: &SlotModel, coder: Coder, st: &mut LaneState<'_>, ctx: usize) -> u32 {
    st.r.refill();
    let tok = match coder {
        Coder::Huffman { .. } => sm.huff[ctx].decode(&mut st.r),
        Coder::Tans { .. } => {
            let e = sm.tans[ctx].dec[st.x as usize];
            let nb = (e >> 8) & 15;
            st.x = (e >> 16) + st.r.read(nb);
            e & 255
        }
    };
    let vt = sm.vlut.len() as u32;
    if tok < (1 << sm.tc.k) {
        tok
    } else if tok < vt {
        let (b, nb) = sm.vlut[tok as usize];
        b + st.r.read(nb)
    } else {
        let (b, nb) = sm.rlut[(tok - vt) as usize];
        let len = b + st.r.read(nb) + 1;
        st.run = len - 1;
        0
    }
}

/// Decodes one stripe lane by lane (ILP only across slots).
pub fn decode_lane_seq(enc: &LabEncoded, si: usize, geom: &LaneStripe, out: &mut [Vec<u16>]) {
    let cfg = enc.cfg;
    let ns = enc.slots.len();
    for l in 0..geom.widths.len() {
        let off = geom.lane_offset(l);
        let lw = geom.widths[l];
        let n = lw * geom.rows;
        let mut sts: Vec<LaneState<'_>> = (0..ns)
            .map(|s| {
                let mut r = BitReader::new(&enc.streams[si][l][s]);
                let x = match cfg.coder {
                    Coder::Tans { log_m } => r.read_refill(log_m),
                    _ => 0,
                };
                LaneState { r, x, run: 0 }
            })
            .collect();
        for j in 0..n {
            for s in 0..ns {
                let st = &mut sts[s];
                let u = if st.run > 0 {
                    st.run -= 1;
                    0
                } else {
                    let sm = &enc.slots[s];
                    let mut sum = ctx_sum_lane(&out[s], off, lw, j);
                    if cfg.cross && s >= 1 && s < cfg.color_slots {
                        sum += out[0][off + j] as u32;
                    }
                    let ctx = sm.ctx_map[ctx_class(sum)] as usize;
                    decode_one(sm, cfg.coder, st, ctx)
                };
                out[s][off + j] = u as u16;
            }
        }
    }
}

/// Decodes one stripe with all lanes in lockstep (ILP across lanes x slots).
pub fn decode_lockstep(enc: &LabEncoded, si: usize, geom: &LaneStripe, out: &mut [Vec<u16>]) {
    let cfg = enc.cfg;
    let ns = enc.slots.len();
    let nl = geom.widths.len();
    let mut sts: Vec<LaneState<'_>> = Vec::with_capacity(nl * ns);
    for l in 0..nl {
        for s in 0..ns {
            let mut r = BitReader::new(&enc.streams[si][l][s]);
            let x = match cfg.coder {
                Coder::Tans { log_m } => r.read_refill(log_m),
                _ => 0,
            };
            sts.push(LaneState { r, x, run: 0 });
        }
    }
    let offs: Vec<usize> = (0..nl).map(|l| geom.lane_offset(l)).collect();
    let maxn = geom.widths.iter().map(|w| w * geom.rows).max().unwrap_or(0);
    for j in 0..maxn {
        for l in 0..nl {
            let lw = geom.widths[l];
            if j >= lw * geom.rows {
                continue;
            }
            let off = offs[l];
            for s in 0..ns {
                let st = &mut sts[l * ns + s];
                let u = if st.run > 0 {
                    st.run -= 1;
                    0
                } else {
                    let sm = &enc.slots[s];
                    let mut sum = ctx_sum_lane(&out[s], off, lw, j);
                    if cfg.cross && s >= 1 && s < cfg.color_slots {
                        sum += out[0][off + j] as u32;
                    }
                    let ctx = sm.ctx_map[ctx_class(sum)] as usize;
                    decode_one(sm, cfg.coder, st, ctx)
                };
                out[s][off + j] = u as u16;
            }
        }
    }
}

// ---- optimized scalar kernels ------------------------------------------------

#[inline(always)]
fn decode_tok_huff(sm: &SlotModel, st: &mut LaneState<'_>, ctx: usize) -> u32 {
    let t = &sm.huff[ctx];
    let idx = (st.r.peek() & ((1u64 << t.bits) - 1)) as usize;
    let e = t.entries[idx];
    st.r.consume((e & 15) as u32);
    (e >> 4) as u32
}

#[inline(always)]
fn decode_tok_tans(sm: &SlotModel, st: &mut LaneState<'_>, ctx: usize) -> u32 {
    let e = sm.tans[ctx].dec[st.x as usize];
    let nb = (e >> 8) & 15;
    st.x = (e >> 16) + st.r.read(nb);
    e & 255
}

#[inline(always)]
fn finish_tok(sm: &SlotModel, st: &mut LaneState<'_>, tok: u32, direct: u32, vt: u32) -> u32 {
    if tok < direct {
        tok
    } else if tok < vt {
        let (b, nb) = sm.vlut[tok as usize];
        b + st.r.read(nb)
    } else {
        let (b, nb) = sm.rlut[(tok - vt) as usize];
        let len = b + st.r.read(nb) + 1;
        st.run = len - 1;
        0
    }
}

/// Specialized lane-sequential decoder: `NS` slots, `TANS` selects the coder.
pub fn decode_fast<const NS: usize, const TANS: bool>(
    enc: &LabEncoded,
    si: usize,
    geom: &LaneStripe,
    out: &mut [Vec<u16>],
) {
    let cfg = enc.cfg;
    let log_m = match cfg.coder {
        Coder::Tans { log_m } => log_m,
        _ => 0,
    };
    let sms: [&SlotModel; NS] = core::array::from_fn(|s| &enc.slots[s]);
    let directs: [u32; NS] = core::array::from_fn(|s| 1 << sms[s].tc.k.min(sms[s].tc.depth));
    let vts: [u32; NS] = core::array::from_fn(|s| sms[s].vlut.len() as u32);
    let cross: [bool; NS] = core::array::from_fn(|s| cfg.cross && s >= 1 && s < cfg.color_slots);
    // ctx lookup by bit length of the sum (0..=32).
    let ctxl: [[u8; 33]; NS] =
        core::array::from_fn(|s| core::array::from_fn(|b| sms[s].ctx_map[b.min(CTX_CLASSES - 1)]));
    let (o0, rest) = out.split_at_mut(1);
    let mut planes: [&mut [u16]; NS] = {
        let mut it = core::iter::once(&mut o0[0][..]).chain(rest.iter_mut().map(|v| &mut v[..]));
        core::array::from_fn(|_| it.next().unwrap())
    };
    for l in 0..geom.widths.len() {
        let off = geom.lane_offset(l);
        let lw = geom.widths[l];
        let mut sts: [LaneState<'_>; NS] = core::array::from_fn(|s| {
            let mut r = BitReader::new(&enc.streams[si][l][s]);
            let x = if TANS { r.read_refill(log_m) } else { 0 };
            LaneState { r, x, run: 0 }
        });
        for row in 0..geom.rows {
            let ro = off + row * lw;
            let mut uw = [0u32; NS];
            for x in 0..lw {
                let mut u_now = [0u32; NS];
                for s in 0..NS {
                    let st = &mut sts[s];
                    let u = if st.run > 0 {
                        st.run -= 1;
                        0
                    } else {
                        let p = &planes[s];
                        let un = if row > 0 { p[ro + x - lw] as u32 } else { 0 };
                        let mut sum = match (x > 0, row > 0) {
                            (true, true) => uw[s] + un,
                            (true, false) => 2 * uw[s],
                            (false, true) => 2 * un,
                            (false, false) => 0,
                        };
                        if cross[s] {
                            sum += u_now[0];
                        }
                        let ctx = ctxl[s][(32 - sum.leading_zeros()) as usize] as usize;
                        st.r.refill();
                        let tok = if TANS {
                            decode_tok_tans(sms[s], st, ctx)
                        } else {
                            decode_tok_huff(sms[s], st, ctx)
                        };
                        finish_tok(sms[s], st, tok, directs[s], vts[s])
                    };
                    u_now[s] = u;
                    uw[s] = u;
                    planes[s][ro + x] = u as u16;
                }
            }
        }
    }
}

/// Lossy decode time per stage since the last call, in ns summed over
/// threads: DC image, entropy, IDCT, color.
#[cfg(feature = "std")]
pub fn lossy_stage_ns() -> [u64; 6] {
    crate::lossy::prof::take()
}

/// Lossy encoder token counts since the last call: (tokens, lockstep lane
/// slots = 16 x longest lane, summed over stripes).
#[cfg(feature = "std")]
pub fn lossy_token_stats() -> [u64; 2] {
    crate::lossy::prof::take_tokens()
}

/// LAB: lossless multi-row entropy kernels: 0 = one slot at a time,
/// 1 = slots 1 + 2 paired, 2 (default) = also slot 0 of the next row.
pub fn set_lossless_row_pairs(level: u8) {
    crate::v1::ROW_PAIRS.store(level, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: value-table channel maps (SPEC 2.1 map mode 9): 0 (default) where
/// estimated smaller, 1 never, 2 on every candidate channel.
pub fn set_value_tables(mode: u8) {
    crate::v1::VALUE_TABLES.store(mode, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: the encoder's largest value table for 16- and 32-bit channels
/// (0 restores the default, 2^14; at most 2^16), for decode-speed studies
/// of table size.
pub fn set_value_table_cap(entries: usize) {
    crate::encode::tables::LAB_CAP.store(entries, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: (map mode, bits) of each channel of a lossless file.
pub fn channel_maps(
    data: &[u8],
) -> Result<alloc::vec::Vec<(alloc::string::String, u8)>, crate::Error> {
    if crate::parse::header::header_version(data)? == 4 {
        let session = crate::decode_resources::DecodeSession::new(
            crate::decode_resources::DecodePolicy::default()
                .with_output_limits(crate::Limits::NONE),
        );
        let file = crate::v4::NumericFile::open(
            data,
            crate::v4::TileLimits {
                max_object_bytes: u64::MAX,
                max_tile_bytes: u64::MAX,
            },
            &session,
        )?;
        let mut maps = None;
        file.visit_groups(|work| {
            if maps.is_none() {
                maps = Some(work.inspection_maps());
            }
            Ok(())
        })?;
        return maps.ok_or(crate::Error::Corrupt("V4 numeric groups missing"));
    }
    let p = crate::v1::parse(data)?;
    Ok(p.layout
        .maps
        .iter()
        .map(|m| (alloc::format!("{:?}", m.mode), m.bits))
        .collect())
}

/// LAB: palette stripes and all stripes of a lossless file.
pub fn palette_stripes(data: &[u8]) -> Result<(usize, usize), crate::Error> {
    let p = crate::v1::parse(data)?;
    let pal = p
        .stripes
        .iter()
        .filter(|s| s.first() == Some(&crate::v1::PAL_STRIPE))
        .count();
    Ok((pal, p.stripes.len()))
}

/// LAB: entropy-work counts of the last lossy encode: [run-level tokens,
/// coded groups, block-channels with any coded group].
#[cfg(feature = "std")]
pub fn lossy_side_stats() -> [u64; 3] {
    crate::lossy::prof::take_side_stats()
}

/// LAB: the AC entropy-table section of the file the last
/// [`crate::ImiEncoderConfig::encode`] on this thread wrote, as
/// `[entropy mode byte, table payload bytes]` (mode 0 run-level .. 3
/// split-Huffman; `[255, 0]` when no lossy encode ran on this thread). The
/// table bytes are the length-prefixed blob between the parameter envelope
/// and the nested-image flags (what a seed bank would replace), excluding
/// its varint length prefix. The reading is consumed: a second call is
/// `[255, 0]` (split-bound study, 2026-10-10).
#[cfg(feature = "std")]
pub fn lossy_table_stats() -> [u64; 2] {
    crate::lossy::prof::take_ac_tables()
}

/// LAB: the entropy mode (0 run-level .. 3 split-Huffman) and AC
/// entropy-table payload bytes of a lossy file, parsed back from its own
/// grammar (the check [`lossy_table_stats`]'s encode-side counter is held
/// to).
#[cfg(feature = "lab")]
pub fn lossy_ac_table_bytes(file: &[u8]) -> Result<(u8, u64), crate::Error> {
    crate::lossy::lab_ac_table_bytes(file).map(|(e, n)| (e as u8, n))
}

/// LAB: force the AVX-512 run-level entropy kernel (hardware gathers or
/// software lookups) for A/B timing.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
pub fn set_avx512_gather(on: bool) {
    crate::lossy::force_gather(Some(on));
}

/// LAB: per-lane scalar entropy decoding vs the production kernel on a
/// lossy file (timings + agreement check).
#[cfg(feature = "std")]
pub fn lossy_lane_bench(data: &[u8], iters: usize) -> alloc::string::String {
    crate::lossy::lanes_lab::bench(data, iters)
}

/// LAB: token mix and EOB distribution of a lossy file.
#[cfg(feature = "std")]
pub fn lossy_token_mix(data: &[u8]) -> alloc::string::String {
    crate::lossy::lanes_lab::stats(data)
}

/// LAB: per-channel histogram of split-Huffman coded-group counts (eobs)
/// by parsing the lane stream with the production grammar; unsupported
/// for other entropy modes.
#[cfg(feature = "std")]
pub fn lossy_counts_histogram(data: &[u8]) -> Result<alloc::string::String, crate::Error> {
    crate::lossy::lanes_lab::counts_histogram(data)
}

/// LAB: dumps a lossy file's symbol streams for external coder benchmarks.
#[cfg(feature = "std")]
pub fn lossy_dump_streams(data: &[u8], dir: &str, tag: &str) -> alloc::string::String {
    crate::lossy::lanes_lab::dump(data, dir, tag)
}

/// LAB: superblock statistics of a lossy file: [parents, parents of 16x16,
/// 16x8 and 8x16 blocks, partition map bytes with their length varints].
pub fn lossy_superblock_stats(data: &[u8]) -> Result<[u64; 5], crate::Error> {
    crate::lossy::superblock_stats_lab(data)
}

/// LAB: share of fully-flat blocks (every channel `eobs` zero).
#[cfg(feature = "std")]
pub fn lossy_flat_share(data: &[u8]) -> Result<alloc::string::String, crate::Error> {
    crate::lossy::lanes_lab::flat_share(data)
}

/// LAB: lossy decode stage timers on/off (on by default).
#[cfg(feature = "std")]
pub fn set_stage_timers(on: bool) {
    crate::lossy::prof::ON.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: how lossy decodes on more than one worker (other than
/// `DecodeSession`) schedule DC stripes that span several lossy stripes.
/// 0: each work item decodes the DC stripe
/// it needs. 1: every DC stripe decoded once, by the first item that needs
/// it, and handed to its lossy stripes as its rows are decoded. 2 (default):
/// as 1, but each DC stripe is decoded by its own work item, and the lossy
/// items wait for DC rows only after their entropy decode. Process-global.
#[cfg(feature = "std")]
pub fn set_shared_dc(mode: u8) {
    crate::lossy::set_shared_dc(mode);
}

/// LAB: lossy stripe pairing for the two-stripe entropy kernel (0
/// automatic, 1 always, 2 never).
pub fn set_stripe_pairs(v: u8) {
    crate::lossy::set_pairs(v);
}

/// LAB: keep lossy per-thread scratch buffers across decodes (no fresh
/// page faults per decode).
#[cfg(feature = "std")]
pub fn set_keep_scratch(on: bool) {
    crate::lossy::set_keep_scratch(on);
}

/// LAB: lossy stripes per work item within one nested DC stripe (0 =
/// automatic, see `split_dc_groups`).
pub fn set_dc_split(n: usize) {
    crate::lossy::set_dc_split(n);
}

/// LAB: lossy reconstruction chunk width in blocks (0 = default,
/// `usize::MAX` = whole block rows).
pub fn set_recon_chunk(n: usize) {
    crate::lossy::set_recon_chunk(n);
}

/// LAB: override the lossy encoder's DC step multipliers, in 1/64 units
/// (luma plane, chroma planes; 0 keeps the default). The values are written
/// to the file (parameter envelope), so the files decode anywhere.
/// Process-global.
pub fn set_lossy_dc_step(luma: u8, chroma: u8) {
    use core::sync::atomic::Ordering::Relaxed;
    crate::lossy::DC_STEP_OVERRIDE[0].store(luma, Relaxed);
    crate::lossy::DC_STEP_OVERRIDE[1].store(chroma, Relaxed);
}

/// LAB: lossy stripes per DC stripe (0 = automatic, about 8 block rows).
/// Encoder-only geometry: any value decodes. Process-global.
pub fn set_lossy_dc_stripes(k: u32) {
    crate::lossy::DC_STRIPES_LAB.store(k, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: nearest and clamp-aware DC rounding in the lossy encoder (on by
/// default; off restores the shared dead-zone rounding). Process-global.
pub fn set_lossy_dc_nearest(on: bool) {
    crate::lossy::DC_NEAREST.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: a lossy encode that also reports each 8x8 block's run-level AC bits
/// (EOB and AC tokens of the three planes under the image's final tables,
/// `log2(M / freq)` plus raw bits; raster block order) and, with
/// `zero_ac`, drops the AC coefficients of the masked blocks (their DC
/// stays). Requires run-level entropy, no decode-speed tier and no column
/// groups. For the lossless-over-lossy estimate.
#[cfg(feature = "std")]
pub fn encode_lossy_blocks(
    pixels: &[u8],
    stride: usize,
    info: &crate::ImageInfo,
    cfg: &crate::ImiEncoderConfig,
    zero_ac: Option<&[bool]>,
) -> Result<(alloc::vec::Vec<u8>, alloc::vec::Vec<f32>), crate::Error> {
    let cfg = &lossy_of(cfg)?;
    use crate::lossy::block_lab::{AC_ZERO, BLOCK_BITS};
    let (bw, bh) = (
        info.width.div_ceil(8) as usize,
        info.height.div_ceil(8) as usize,
    );
    if cfg.entropy != crate::LossyEntropy::RunLevel
        || cfg.decode_speed.is_some()
        || crate::lossy::lossy_group_cols(cfg, info.width as usize) != 0
        || zero_ac.is_some_and(|m| m.len() != bw * bh)
    {
        return Err(crate::Error::Unsupported(
            "block report needs run-level entropy, no tier, no column groups",
        ));
    }
    AC_ZERO.with(|m| *m.borrow_mut() = zero_ac.map(|z| z.to_vec()));
    BLOCK_BITS.with(|b| *b.borrow_mut() = Some(alloc::vec::Vec::new()));
    // The plain lossy file: no exact blocks.
    let mut cfg = cfg.clone();
    cfg.exact_blocks = Some(0.0);
    let file = crate::lossy::encode_lossy_with_progress(
        pixels,
        stride,
        info,
        &crate::Metadata::default(),
        &cfg,
        &how_far::NoPulse,
    );
    AC_ZERO.with(|m| *m.borrow_mut() = None);
    let bits = BLOCK_BITS
        .with(|b| b.borrow_mut().take())
        .unwrap_or_default();
    let file = file?;
    if bits.len() != bw * bh {
        return Err(crate::Error::Unsupported(
            "block report: no run-level coding",
        ));
    }
    Ok((crate::v4::migrate::finish_encoded(file)?, bits))
}

/// LAB: exact 8x8 blocks over lossy with ratio `ratio` and no decode-cost
/// budget, overriding [`crate::LossyTuning::exact_blocks`] and the tier
/// default for every encode (nested stripe height from `IMIJ_EXACT_ROWS`,
/// else the tier's). 0 = no override. Process-global; change only between
/// calls.
#[cfg(feature = "std")]
pub fn set_lossy_exact_blocks(ratio: f32) {
    crate::lossy::block_lab::EXACT_RATIO
        .store(ratio.to_bits(), core::sync::atomic::Ordering::Relaxed);
}

/// LAB: [`set_lossy_exact_blocks`] for one encode on this thread only (for
/// harnesses that encode on several threads at once).
#[cfg(feature = "std")]
pub fn encode_lossy_exact(
    pixels: &[u8],
    stride: usize,
    info: &crate::ImageInfo,
    cfg: &crate::ImiEncoderConfig,
    ratio: f32,
) -> Result<alloc::vec::Vec<u8>, crate::Error> {
    let cfg = &lossy_of(cfg)?;
    use crate::lossy::block_lab::EXACT_RATIO_HERE;
    EXACT_RATIO_HERE.with(|c| c.set(Some(ratio)));
    let r = crate::lossy::encode_lossy_with_progress(
        pixels,
        stride,
        info,
        &crate::Metadata::default(),
        cfg,
        &how_far::NoPulse,
    );
    EXACT_RATIO_HERE.with(|c| c.set(None));
    r
}

/// LAB: overshoot deringing in the lossy encoder (on by default).
/// Process-global; change only between calls.
pub fn set_lossy_dering(on: bool) {
    crate::lossy::dering::ENABLED.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: forward DCT: 0 = factored AAN with FMA, 1 = dense reference,
/// 2 = factored AAN without FMA. Process-global; change only between calls.
pub fn set_fdct_mode(mode: u8) {
    crate::lossy::enc_simd::FDCT_MODE.store(mode, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: evaluate a fused SIMD inverse. This changes decoded pixels and is
/// not the normative inverse. The portable one-rounding operations also cover
/// scalar and WASM; ordinary `mul_add` has a different backend contract.
pub fn set_idct_fma(on: bool) {
    crate::lossy::IDCT_FMA.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: the automatic AVX-512 entropy-kernel choice on this CPU (true =
/// hardware gathers); None without AVX-512.
#[cfg(all(feature = "std", target_arch = "x86_64"))]
pub fn avx512_kernel_choice() -> Option<alloc::string::String> {
    crate::lossy::kernel_choice_str()
}

/// LAB: the `zen-codestream-trace/0` JSON of `data`, produced by the
/// streaming reader as it decodes (spans, fields, record image regions,
/// read order), with structural self-checks. See docs/CODESTREAM_VIS.md.
#[cfg(feature = "std")]
pub fn codestream_trace(data: &[u8]) -> alloc::string::String {
    crate::trace::document(data)
}

/// [`crate::decode_into_threads`] reporting each lossless or lossy unit
/// (stripe, or stripe and column group) to a live `pulse` as one child
/// phase, started and finished by the worker that decodes it; the two
/// stripes of a lossy pair start and finish together. Child names follow the
/// codestream trace (`stripe 3 group 1 (rows 192-255, columns 64-127)`).
/// Native JPEG payloads decode as usual and report no units.
#[cfg(feature = "std")]
pub fn decode_into_threads_with_pulse(
    data: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    pulse: &dyn how_far::Pulse,
) -> Result<crate::ImageInfo, crate::Error> {
    crate::v1::decode_into_threads_observed(data, out, stride, threads, pulse)
}

/// LAB: the effort trellis's per-block lambda factor m^k for AQ step
/// multipliers m (0 = off, the default). Process-global; change only
/// between calls.
pub fn set_trellis_aq_lambda_exp(k: f32) {
    crate::lossy::trellis::AQ_LAMBDA_EXP.store(k.to_bits(), core::sync::atomic::Ordering::Relaxed);
}

/// LAB: per-channel offsets of the AC trellis's `lambda_log_scale1`
/// (channel 0 luma, 1 and 2 chroma; 0 = none, the default): +1 doubles the
/// channel's distortion weight against bits (the trellis keeps more of its
/// coefficients). Process-global; change only between calls.
pub fn set_trellis_channel_shift(shift: [f32; 3]) {
    for (slot, d) in crate::lossy::trellis::CH_SHIFT.iter().zip(shift) {
        slot.store(d.to_bits(), core::sync::atomic::Ordering::Relaxed);
    }
}

/// LAB: whether the effort's trellis runs on XYB split-Huffman files
/// (explicit `SplitHuf`, tier images the cost model sends to split-Huffman,
/// small tier images' split-Huffman candidate from the pass-1 coefficients),
/// priced with split-Huffman code lengths (group counts, widths, magnitude
/// and sign bits). On by default; `false` restores split-Huffman without the
/// trellis. Process-global; change only between calls.
pub fn set_trellis_huf_rate(on: bool) {
    crate::lossy::trellis::HUF_RATE.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: a lossy file's coefficients (slot 0: the quantized DC): (block width, block
/// height, coefficients at `(block * 3 + channel) * 64 + natural index`, AC
/// record bytes, DC record bytes). One column group, default layouts.
#[allow(clippy::type_complexity)]
pub fn lossy_quantized(
    data: &[u8],
) -> Result<(usize, usize, Vec<i32>, usize, usize), crate::Error> {
    crate::lossy::quantized_lab(data)
}

/// LAB: assemble a lossy file from already-quantized coefficients — the
/// encoder's entropy coders and nested DC image over caller-supplied 8x8
/// coefficients instead of its transform and quantizer output (a
/// coefficient-import study hook; `U8` `Rgb` or `Gray` image info only).
///
/// `q` covers the `ceil(w/8) x ceil(h/8)` block grid: at
/// `(block * 3 + channel) * 64 + natural index` it holds the quantized
/// coefficients with the quantized DC at index 0; the zigzag nonzero masks
/// the entropy coder emits are derived from it.
///
/// The stages that would recompute or edit coefficients must be off in
/// `cfg` (AQ, subblock layouts, the trellis, RDOQ, LF prediction, exact
/// blocks, superblocks, a decoder-memory plan) — checked, returning
/// `Unsupported`. The color transform and quantization the decoder applies
/// come from `cfg` as in a pixel encode: for imported JPEG coefficients the
/// settings are `quality` 50 (scale 1), `channel_mul` `[1, 1, 1]`, the lab
/// `dc_step` `[64, 64]` (unit DC steps), `lab.explicit_matrix` the JPEG
/// tables, `lab.color_transform` the centered YCbCr matrix, YCoCg (the
/// hook's fixed color) and `entropy`/`effort` the coders to measure.
/// [`lossy_quantized`] reads the coefficients back for the round-trip
/// oracle.
pub fn encode_lossy_quantized(
    info: &crate::ImageInfo,
    meta: &crate::Metadata,
    cfg: &crate::ImiEncoderConfig,
    q: &[i32],
) -> Result<alloc::vec::Vec<u8>, crate::Error> {
    encode_lossy_quantized_lowered(info, meta, &lossy_of(cfg)?, q)
        .and_then(crate::v4::migrate::finish_encoded)
}

pub(crate) fn encode_lossy_quantized_lowered(
    info: &crate::ImageInfo,
    meta: &crate::Metadata,
    cfg: &crate::LossyConfig,
    q: &[i32],
) -> Result<alloc::vec::Vec<u8>, crate::Error> {
    let file =
        crate::container::finish_unit_table(crate::lossy::encode_lossy_coefs(info, meta, cfg, q)?)?;
    let file = crate::container::locate(file)?;
    crate::checksum::finish_file(file, cfg.checksum.then_some(cfg.hash))
}

/// LAB: reconstruct one JPEG coefficient block through the normative AAN
/// kernel and its production dequantization factors. Inputs are natural-order
/// signed coefficients and the component's JPEG table. Output stays centered;
/// the reconstruction study controls any plane clamp and chroma upsampling.
#[cfg(feature = "native-jpeg")]
pub fn jpeg_idct(q: &[i16; 64], table: &[u16; 64]) -> [f32; 64] {
    crate::lossy::jpeg_idct_lab(q, table)
}

/// LAB: the native component plane-wide triangle, with strided input/output.
/// This retains the measured f32 arithmetic and crops a final odd row/column.
#[cfg(feature = "native-jpeg")]
pub fn jpeg_triangle(
    input: &[f32],
    width: usize,
    height: usize,
    stride: usize,
    output: &mut [f32],
    out_width: usize,
    out_height: usize,
    out_stride: usize,
) -> Result<(), crate::Error> {
    crate::lossy::jpeg::triangle(
        input, width, height, stride, output, out_width, out_height, out_stride,
    )
}

/// LAB: encode one retained native component's natural-order signed spectrum.
/// Width/height cover complete coded 8x8 blocks; source identity/sampling/siting
/// belong to the native descriptor, rather than this Gray entropy carrier.
#[cfg(feature = "native-jpeg")]
pub fn encode_jpeg_component(
    width: u32,
    height: u32,
    table: &[u16; 64],
    coefficients: &[i16],
    entropy: crate::LossyEntropy,
    effort: f32,
) -> Result<Vec<u8>, crate::Error> {
    crate::lossy::encode_native_component(width, height, table, coefficients, entropy, effort)
}

/// LAB: strict native component coefficient readback, checking the expected
/// grid/table and rejecting coefficient-modifying declarations or hidden values.
#[cfg(feature = "native-jpeg")]
pub fn decode_jpeg_component(
    file: &[u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
) -> Result<Vec<i16>, crate::Error> {
    crate::lossy::decode_native_component(file, width, height, table)
}

/// LAB: apply the record-17 explicit matrix with unit channel scales and the
/// normative output clamp and nearest-even rounding. One pixel, no row layout.
pub fn linear_rgb(matrix: &[[f32; 4]; 3], values: [f32; 3]) -> [u8; 3] {
    crate::lossy::linear_rgb_lab(matrix, values)
}

/// LAB (coefgap study, 2026-10-09): run-level decodes do the extra work of
/// EOB contexts split by a per-block luma DC-gradient bucket (a decode-cost
/// proxy; coefficients stay exact). Process-global; AVX-512 and NEON kernels.
pub fn set_eob_bucket_proxy(on: bool) {
    crate::lossy::EOB_BKT_PROXY.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: [`set_eob_bucket_proxy`]'s mode: 1 (default) the per-stripe bucket
/// pass and the kernels' bucket loads, 2 the pass only, 3 the loads only.
pub fn set_eob_bucket_proxy_mode(mode: u32) {
    crate::lossy::EOB_BKT_PROXY_MODE.store(mode, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: [`set_eob_bucket_proxy`]'s liveness probe: every bucket offset
/// becomes `probe` (0 = exact decodes; nonzero breaks them, proving the
/// kernels read the buckets).
pub fn set_eob_bucket_probe(probe: u32) {
    crate::lossy::EOB_BKT_PROBE.store(probe, core::sync::atomic::Ordering::Relaxed);
}

/// LAB (coefgap study, 2026-10-09): the lossy encoder merges its EOB and AC
/// tables (after clustering at the effort's budget) down to the number that
/// codes smallest, tables plus rANS data bits (encoder-only; decoders read any
/// table count). Process-global; change only between encodes. Same as
/// [`set_table_k_mode`] 1 (on) or 0 (off: the effort's budgets, also at the
/// efforts where production chooses counts by coded size).
pub fn set_table_k_search(on: bool) {
    set_table_k_mode(on as u8);
}

/// LAB (coefgap study, 2026-10-09): overrides how the lossy encoder picks its
/// table counts, for every coder: 0 the effort's budgets (the A/B switch: main
/// before the production switch), 1 the budgets' clusters merged down to the
/// count that codes smallest (estimated, unseeded), 2 the counts whose seeded
/// tables cost the fewest written bits plus data bits (run-level: AC 1, 2, 4,
/// 8 or the budget, then EOB 1, 2 or the budget; split-Huffman: width 1, 2, 4,
/// 8, 16 or the budget, then count 1, 2, 4 or the budget), 3 mode 2 only when
/// the budgets' table bits are at least 5% of their table plus data bits. The
/// production rule, which [`reset_table_k_mode`] restores, is mode 3 from
/// effort 7.5 for run-level and split-Huffman and the budgets otherwise. Only
/// the tables a file writes; rate estimates (exact-block selection) keep the
/// budgets. Values above 3 act as 3. Encoder-only. Process-global; change
/// only between encodes.
pub fn set_table_k_mode(mode: u8) {
    crate::lossy::TABLE_K_SEARCH.store(mode.min(3), core::sync::atomic::Ordering::Relaxed);
}

/// LAB: drops a [`set_table_k_mode`] override (the production rule).
/// Process-global; change only between encodes.
pub fn reset_table_k_mode() {
    crate::lossy::TABLE_K_SEARCH.store(
        crate::lossy::TABLE_K_DEFAULT,
        core::sync::atomic::Ordering::Relaxed,
    );
}

/// LAB (coefgap study, 2026-10-09): lanes of the nested DC image (1..=16;
/// 0 = the lossless encoder's default). Process-global; change only between
/// encodes. Encoder-only: decoders read any lane count.
pub fn set_lossy_dc_lanes(n: u32) {
    crate::lossy::DC_LANES_LAB.store(n.min(16), core::sync::atomic::Ordering::Relaxed);
}

/// LAB (coefgap study, 2026-10-09): IMI's run-level table machinery on
/// arbitrary raw-context histograms (`hist[ctx]`, at most 128 symbols each):
/// contexts `..n_first` cluster into at most `max_first` tables, the rest into
/// at most `max_rest`; every table chosen and costed as the writer codes it.
/// Returns (context-map bits, table bits, rANS data bits, ideal data bits,
/// tables).
pub fn table_cost(
    hist: &[Vec<u64>],
    n_first: usize,
    max_first: usize,
    max_rest: usize,
) -> (u64, u64, f64, f64, usize) {
    crate::lossy::coef::lab_table_cost(hist, n_first, max_first, max_rest)
}

/// LAB: the nested DC image of a lossy file: its info (3 or 4 channels:
/// per-block DC of each plane, then the AQ channel), packed samples and
/// stripe height in blocks.
pub fn lossy_dc_image(data: &[u8]) -> Result<(crate::ImageInfo, Vec<u8>, u32), crate::Error> {
    crate::lossy::dc_image_lab(data)
}

/// LAB: on this thread, write multi-unit payloads without the unit table
/// hint (SPEC 2.7: readers then find records by their own lengths), as a
/// streaming writer that cannot buffer its records must. Tests and
/// benchmarks compare the two layouts with it.
#[cfg(feature = "std")]
pub fn set_length_prefixed_layout(on: bool) {
    set_unit_table_form(if on {
        UnitTableForm::None
    } else {
        UnitTableForm::Hint
    });
}

pub use crate::v1::UnitTableForm;

/// LAB: on this thread, store each payload's unit table as `form` (SPEC
/// 2.7): the hint (the default), none, check tail with a trailing `uIDX`
/// chunk, or a head reservation filled, left empty or marked check tail.
/// The writer paths of streaming and seekable encoders, for tests and
/// region-read measurements.
#[cfg(feature = "std")]
pub fn set_unit_table_form(form: UnitTableForm) {
    crate::v1::UNIT_TABLE.with(|c| c.set(form));
}

/// LAB: `IMIJ_CSUM_PIECE_LOG2` (10..=40) overrides the file checksum piece
/// size encoders choose by file size.
#[cfg(feature = "std")]
pub(crate) fn csum_piece_log2() -> Option<u8> {
    std::env::var("IMIJ_CSUM_PIECE_LOG2")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|k| (crate::checksum::MIN_PIECE_LOG2..=crate::checksum::MAX_PIECE_LOG2).contains(k))
}

/// LAB: `IMIJ_CSUM_WIDTH` (4, 8 or the algorithm's width) overrides the digest
/// bytes per piece.
#[cfg(feature = "std")]
pub(crate) fn csum_width() -> Option<u8> {
    std::env::var("IMIJ_CSUM_WIDTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|w| matches!(w, 4 | 8 | 16 | 32))
}

/// LAB: `IMIJ_CSUM_SCHED=prepass|queue|jit` picks how a multi-threaded
/// decode checks file checksum pieces (default jit).
#[cfg(feature = "std")]
pub(crate) fn csum_schedule() -> Option<crate::v1::Schedule> {
    match std::env::var("IMIJ_CSUM_SCHED").ok()?.as_str() {
        "prepass" => Some(crate::v1::Schedule::Prepass),
        "queue" => Some(crate::v1::Schedule::Queue),
        "jit" => Some(crate::v1::Schedule::Jit),
        _ => None,
    }
}

/// Small-transform study: nanoseconds per block plane of a decoder kernel
/// (`aan8`, `f4`, `int4`, `ref4`, `ref8`, `dense4`, `copy`; see
/// `lossy::small_lab::kernel_bench`).
#[cfg(feature = "std")]
pub fn small_kernel_bench(
    kernel: &str,
    blocks: usize,
    iters: usize,
    rounds: usize,
) -> Result<f64, crate::Error> {
    crate::lossy::small_lab::kernel_bench(kernel, blocks, iters, rounds)
}

/// Small-transform statistics of a lossy file: [parents, merged parents,
/// 4x4 planes of plane 0, 1, 2, partition map bytes, AC record bytes].
pub fn lossy_small_stats(data: &[u8]) -> Result<[u64; 7], crate::Error> {
    crate::lossy::small_stats_lab(data)
}

/// Small-transform map grammar study: flag-section bytes of a file's maps
/// under seven coding variants (`lossy::small_lab::map_variants`).
#[cfg(feature = "std")]
pub fn small_map_variants(data: &[u8]) -> Result<[u64; 11], crate::Error> {
    crate::lossy::small_lab::map_variants(data)
}

/// Per lossy unit (record order), per parent, the planes with AC (bit c):
/// those that carry split flags in a joint partition map (SPEC 2.11.1).
pub fn lossy_unit_ac_bits(data: &[u8]) -> Result<Vec<Vec<u8>>, crate::Error> {
    crate::lossy::unit_ac_bits_lab(data)
}

/// The lossy coder's settings of a public configuration; a lossless coder is
/// refused.
fn lossy_of(cfg: &crate::ImiEncoderConfig) -> Result<crate::LossyConfig, crate::Error> {
    match cfg.lower() {
        crate::config::Lowered::Lossy(c) => Ok(c),
        _ => Err(crate::Error::Unsupported(
            "lab lossy entry point needs a lossy coder",
        )),
    }
}
