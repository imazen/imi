#![allow(
    dead_code,
    unused_mut,
    unused_variables,
    private_interfaces,
    clippy::all
)]
//! Lab: the original Huffman-coded prototype encoder (size accounting only;
//! no decoder). Kept for entropy-layer comparisons.

use alloc::vec;
use alloc::vec::Vec;

use crate::bits::BitWriter;
use crate::coding_layout::{CTX_CLASSES, MAX_CTX, ctx_class};
use crate::encode::{Analysis, AnalysisConfig, analyze, choose_ctx_map};
use crate::huffman::{self, MAX_LEN};
use crate::tokens::TokenCfg;
use crate::{Error, ImageInfo, Metadata, put_varint};

#[derive(Clone, Copy)]
struct Tok {
    class: u8,
    tok: u16,
    nb: u8,
    extra: u32,
}

#[allow(dead_code)]
#[inline(always)]
fn sgn(v: u32, bits: u32) -> i32 {
    let half = 1u32 << (bits - 1);
    if v >= half {
        v as i32 - (1i32 << bits)
    } else {
        v as i32
    }
}

/// Applies a decorrelation mode to per-channel modular residuals
/// (G, R, B order) in place.
#[inline(always)]
#[allow(dead_code)]
pub(crate) fn decor_forward(mode: u8, d: &mut [u32; 3], bits: u32, mask: u32) {
    let (g, r, b) = (d[0], d[1], d[2]);
    match mode {
        0 => {}
        1 => {
            d[1] = r.wrapping_sub(g) & mask;
            d[2] = b.wrapping_sub(g) & mask;
        }
        2 => {
            d[1] = r.wrapping_sub(g) & mask;
            d[2] = b.wrapping_sub(r) & mask;
        }
        3 => {
            d[1] = r.wrapping_sub(g) & mask;
            let avg = (sgn(r, bits) + sgn(g, bits)) >> 1;
            d[2] = b.wrapping_sub(avg as u32) & mask;
        }
        _ => unreachable!(),
    }
}

/// Encodes an image. `pixels` holds `height` rows of `stride` bytes; samples
/// of 16-bit types are native-endian.
pub fn encode_huffman(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &AnalysisConfig,
) -> Result<Vec<u8>, Error> {
    encode_report(pixels, stride, info, meta, cfg).map(|r| r.0)
}

/// Size accounting for entropy-layer experiments.
#[derive(Clone, Debug, Default)]
pub struct SizeReport {
    /// Huffman codeword bits (configured max length).
    pub huff_bits: u64,
    /// Raw extra bits (hybrid token suffixes).
    pub extra_bits: u64,
    /// Ideal tANS bits for log2(states) = 6, 7, 8, 10, 12 (same contexts).
    pub tans_bits: [f64; 5],
    /// Symbols coded (tokens).
    pub tokens: u64,
    /// Max used symbols in any table.
    pub max_used_symbols: usize,
}

pub const TANS_LOGS: [u32; 5] = [6, 7, 8, 10, 12];

/// Encodes and reports entropy-layer accounting.
pub fn encode_report(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &AnalysisConfig,
) -> Result<(Vec<u8>, SizeReport), Error> {
    let a = analyze(pixels, stride, info, cfg)?;
    let Analysis {
        layout,
        res,
        w,
        h,
        hs,
        sw,
        stripe_decor,
        row_pred,
        ..
    } = &a;
    // Lab prototype works on <= 16-bit residuals.
    let res16: Vec<Vec<u16>> = res
        .iter()
        .map(|p| p.iter().map(|&u| u as u16).collect())
        .collect();
    let res = &res16;
    let (w, h, hs, sw) = (*w, *h, *hs, *sw);
    let ns = layout.slot_channel.len();
    let nstripes = h.div_ceil(hs);

    // 4. Tokenize.
    let tcfgs: Vec<TokenCfg> = layout
        .slot_bits
        .iter()
        .map(|&b| {
            TokenCfg::new(
                if b <= 8 { cfg.k_low } else { cfg.k_high },
                cfg.msb_bits,
                b,
                cfg.run_k,
                cfg.run_msb_bits,
            )
        })
        .collect();
    let alpha_sizes: Vec<usize> = tcfgs.iter().map(|t| t.alphabet() as usize).collect();
    let mut hist: Vec<Vec<Vec<u64>>> = (0..ns)
        .map(|s| vec![vec![0u64; alpha_sizes[s]]; CTX_CLASSES])
        .collect();
    let mut toks: Vec<Vec<Vec<Tok>>> = vec![vec![Vec::new(); ns]; nstripes];
    for si in 0..nstripes {
        let y0 = si * hs;
        let y1 = (y0 + hs).min(h);
        for s in 0..ns {
            let cross = cfg.ctx_cross && s >= 1 && s < layout.color_slots;
            let out = &mut toks[si][s];
            tokenize_slot(
                &res[s],
                if cross { Some(&res[0]) } else { None },
                w,
                sw,
                y0,
                y1,
                &tcfgs[s],
                cfg.run_min,
                out,
            );
            for t in out.iter() {
                hist[s][t.class as usize][t.tok as usize] += 1;
            }
        }
    }

    // 5. Context maps and code lengths.
    let max_ctx = (cfg.max_ctx as usize).clamp(1, MAX_CTX);
    let max_len = cfg.max_code_len.clamp(1, MAX_LEN);
    let mut ctx_maps: Vec<[u8; CTX_CLASSES]> = Vec::with_capacity(ns);
    let mut lens: Vec<Vec<Vec<u8>>> = Vec::with_capacity(ns);
    let mut freqs: Vec<Vec<Vec<u64>>> = Vec::with_capacity(ns);
    for s in 0..ns {
        let (map, groups) = choose_ctx_map(&hist[s], max_ctx, max_len);
        let l: Vec<Vec<u8>> = groups
            .iter()
            .map(|f| huffman::build_lengths(f, max_len))
            .collect();
        ctx_maps.push(map);
        lens.push(l);
        freqs.push(groups);
    }

    let mut report = SizeReport::default();
    for s in 0..ns {
        for (g, f) in freqs[s].iter().enumerate() {
            report.huff_bits += huffman::cost_bits(f, &lens[s][g]);
            let used = f.iter().filter(|&&c| c > 0).count();
            report.max_used_symbols = report.max_used_symbols.max(used);
            report.tokens += f.iter().sum::<u64>();
            for (ti, &lm) in TANS_LOGS.iter().enumerate() {
                report.tans_bits[ti] += match crate::ans::normalize(f, lm) {
                    Some(q) => crate::ans::cost_bits(f, &q, lm),
                    None => f64::INFINITY,
                };
            }
        }
    }
    for st in &toks {
        for sl in st {
            report.extra_bits += sl.iter().map(|t| t.nb as u64).sum::<u64>();
        }
    }

    // 6. Serialize.
    let mut out = Vec::with_capacity(w * h);
    crate::container::write_head(
        &mut out,
        info,
        meta,
        crate::Coding::Lossless,
        None,
        crate::DecodeSpeed::Balanced,
    )?;

    let mut data = Vec::with_capacity(w * h);
    for m in &layout.maps {
        data.push(m.mode as u8);
        data.push(m.bits);
        data.extend_from_slice(&m.value.to_le_bytes());
    }
    put_varint(&mut data, hs as u64);
    data.push(cfg.ctx_cross as u8);
    for s in 0..ns {
        data.push(tcfgs[s].k as u8);
        data.extend_from_slice(&ctx_maps[s]);
    }
    let mut tw = BitWriter::new();
    let mut table_refs: Vec<(&[u8], &[u64])> = Vec::new();
    let mut codes: Vec<Vec<Vec<u16>>> = Vec::with_capacity(ns);
    for s in 0..ns {
        for (l, f) in lens[s].iter().zip(&freqs[s]) {
            table_refs.push((l, f));
        }
        codes.push(
            lens[s]
                .iter()
                .map(|l| huffman::canonical_codes(l))
                .collect(),
        );
    }
    let alpha_bits = 9;
    huffman::write_tables(&mut tw, &table_refs, alpha_bits);
    let tbytes = tw.finish();
    put_varint(&mut data, tbytes.len() as u64);
    data.extend_from_slice(&tbytes);

    let mut stripes: Vec<Vec<u8>> = Vec::with_capacity(nstripes);
    for si in 0..nstripes {
        let y0 = si * hs;
        let y1 = (y0 + hs).min(h);
        let mut sb = Vec::new();
        sb.push(stripe_decor[si]);
        for pair in row_pred[y0..y1].chunks(2) {
            sb.push(pair[0] | (pair.get(1).copied().unwrap_or(0) << 4));
        }
        let mut streams: Vec<Vec<u8>> = Vec::with_capacity(ns);
        for s in 0..ns {
            let mut bw = BitWriter::with_capacity(toks[si][s].len());
            let map = &ctx_maps[s];
            for t in &toks[si][s] {
                let c = map[t.class as usize] as usize;
                let l = lens[s][c][t.tok as usize] as u32;
                bw.put(codes[s][c][t.tok as usize] as u32, l);
                if t.nb > 0 {
                    bw.put(t.extra, t.nb as u32);
                }
            }
            streams.push(bw.finish());
        }
        for st in &streams[..ns - 1] {
            put_varint(&mut sb, st.len() as u64);
        }
        for st in &streams {
            sb.extend_from_slice(st);
        }
        stripes.push(sb);
    }
    for sb in &stripes {
        put_varint(&mut data, sb.len() as u64);
    }
    for sb in &stripes {
        data.extend_from_slice(sb);
    }
    crate::container::put_data(&mut out, &data);
    Ok((out, report))
}

/// Neighbors for pixel (x, y) of a coded plane.
#[inline(always)]
pub(crate) fn neighbors(
    p: &[u16],
    w: usize,
    sw: usize,
    x: usize,
    y: usize,
    y0: usize,
) -> (u32, u32, u32, u32) {
    let i = y * w + x;
    let xs = x - x % sw;
    let xe = (xs + sw).min(w);
    if y == y0 {
        let wv = if x > xs { p[i - 1] as u32 } else { 0 };
        return (wv, wv, wv, wv);
    }
    let n = p[i - w] as u32;
    let (wv, nw) = if x > xs {
        (p[i - 1] as u32, p[i - w - 1] as u32)
    } else {
        (n, n)
    };
    let ne = if x + 1 < xe { p[i - w + 1] as u32 } else { n };
    (wv, n, nw, ne)
}

/// Context sum for slot residual at (x, y) within a stripe starting at y0.
#[inline(always)]
pub(crate) fn ctx_sum(r: &[u16], w: usize, sw: usize, x: usize, y: usize, y0: usize) -> u32 {
    let i = y * w + x;
    let uw = if x % sw > 0 {
        Some(r[i - 1] as u32)
    } else {
        None
    };
    let un = if y > y0 { Some(r[i - w] as u32) } else { None };
    match (uw, un) {
        (Some(a), Some(b)) => a + b,
        (Some(a), None) => 2 * a,
        (None, Some(b)) => 2 * b,
        (None, None) => 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn tokenize_slot(
    r: &[u16],
    cross: Option<&Vec<u16>>,
    w: usize,
    sw: usize,
    y0: usize,
    y1: usize,
    tc: &TokenCfg,
    run_min: u32,
    out: &mut Vec<Tok>,
) {
    // Lanes are column segments; each lane is scanned row by row and runs
    // continue across rows inside the lane.
    for xs in (0..w).step_by(sw) {
        let xe = (xs + sw).min(w);
        let lw = xe - xs;
        let n = (y1 - y0) * lw;
        let at = |j: usize| (y0 + j / lw) * w + xs + j % lw;
        let mut j = 0;
        while j < n {
            let i = at(j);
            let x = xs + j % lw;
            let y = y0 + j / lw;
            let mut s = ctx_sum(r, w, sw, x, y, y0);
            if let Some(c) = cross {
                s += c[i] as u32;
            }
            let class = ctx_class(s) as u8;
            let u = r[i] as u32;
            if u == 0 && run_min > 0 {
                let mut len = 1;
                while j + len < n && r[at(j + len)] == 0 {
                    len += 1;
                }
                if len as u32 >= run_min {
                    let (tok, extra, nb) = tc.run(len as u32);
                    out.push(Tok {
                        class,
                        tok: tok as u16,
                        nb: nb as u8,
                        extra,
                    });
                    j += len;
                    continue;
                }
            }
            let (tok, extra, nb) = tc.value(u);
            out.push(Tok {
                class,
                tok: tok as u16,
                nb: nb as u8,
                extra,
            });
            j += 1;
        }
    }
}
