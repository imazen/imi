//! LAB: per-lane scalar entropy decoding, measured against the lockstep
//! SIMD kernels without a format change.
//!
//! The stripe format interleaves the 16 lanes' renormalization words in
//! lockstep decode order, so a lane cannot find its own words without
//! decoding every lane in step. Here a scalar lockstep pass records which
//! lane consumes each word and splits the stream into one word list per
//! lane; a decoder that owns its lane's words can then run lanes
//! independently, `G` lanes interleaved in one loop so the core overlaps
//! their dependency chains. Output (coefficients, EOBs) is compared with
//! the production kernel, then both are timed on the same stripes.

use super::coef::{EOB_CTX, K_CLASSES, MAX_TABLES, NRAW_PAD, REM_CLASSES, ZIGZAG, ZRL, rl_value};
#[cfg(feature = "std")]
use super::{Group, decode_stripe_raw};
use super::{LossyParsed, NCH};
use crate::rans::{LOG_M, M, NL, RANS_L, Words};
#[cfg(feature = "std")]
use alloc::string::String;
use alloc::vec::Vec;

/// One stripe split into per-lane word lists (each padded with zeros).
pub(crate) struct LaneSplit {
    pub x0: [u32; NL],
    pub words: Vec<Vec<u16>>,
    pub active: usize,
    pub bw: usize,
    pub nby: usize,
    pub tokens: usize,
    /// EOB histogram per channel (index = eob 0..=63).
    pub eob_hist: [[u32; 64]; 3],
    /// Token kinds: EOB, ZRL, coefficient by run (0..=3).
    pub kinds: [u32; 6],
    /// Coefficient tokens by magnitude class (0..32).
    pub cls: [u32; 32],
}

/// Scalar lockstep decode of stripe `s` (as `decode_stripe_raw`'s scalar
/// path) recording each word's lane.
pub(crate) fn split(p: &LossyParsed<'_>, s: usize) -> LaneSplit {
    let g = &p.g;
    let (by0, by1) = g.rows(s);
    let (nby, bw) = (by1 - by0, g.bw);
    let active = g.active_lanes();
    let (words, mut x) = Words::from_bytes(p.stripes[s], active).unwrap();
    let x0 = x;
    let mut lanes: Vec<Vec<u16>> = (0..NL).map(|_| Vec::new()).collect();
    let mut wp = 0usize;
    #[derive(Clone, Copy, Default)]
    struct C {
        r: u32,
        bx: u32,
        ch: u8,
        k: u8,
        eob: u8,
        luma: u8,
        done: bool,
    }
    let mut cur = [C::default(); NL];
    for (l, c) in cur.iter_mut().enumerate() {
        c.bx = l as u32;
        c.done = l >= active || nby == 0;
    }
    let mut tokens = 0;
    let mut eob_hist = [[0u32; 64]; 3];
    let mut kinds = [0u32; 6];
    let mut cls = [0u32; 32];
    let mut renorm = |x: &mut u32, wp: &mut usize, l: usize| {
        if *x < RANS_L {
            let w = words.word(*wp);
            lanes[l].push(w);
            *x = (*x << 16) | w as u32;
            *wp += 1;
        }
    };
    let mut sym = [0u32; NL];
    let mut nbits = [0u32; NL];
    let mut live = [false; NL];
    loop {
        let mut any = false;
        for l in 0..NL {
            live[l] = !cur[l].done;
            if !live[l] {
                continue;
            }
            any = true;
            tokens += 1;
            let c = cur[l];
            let ch = c.ch as usize;
            let raw = if c.k == 0 {
                super::coef::eob_ctx(ch, c.luma as u32)
            } else {
                super::coef::ac_ctx(ch, c.k as u32, c.eob as u32)
            };
            let t = p.tables.ctx_map[raw] as usize;
            let xs = x[l];
            let e = p.dec()[(t << LOG_M) | (xs & (M - 1)) as usize];
            let mut nx = ((e & 1023) + 1) * (xs >> LOG_M) + ((e >> 10) & 1023);
            renorm(&mut nx, &mut wp, l);
            x[l] = nx;
            sym[l] = (e >> 20) & 127;
            nbits[l] = e >> 27;
        }
        if !any {
            break;
        }
        let mut extra = [0u32; NL];
        for l in 0..NL {
            if live[l] && nbits[l] > 0 {
                let n = nbits[l].min(16);
                extra[l] = x[l] & crate::mask_of(n);
                x[l] >>= n;
                renorm(&mut x[l], &mut wp, l);
            }
        }
        for l in 0..NL {
            if live[l] && nbits[l] > 16 {
                let n = nbits[l] - 16;
                extra[l] |= (x[l] & crate::mask_of(n)) << 16;
                x[l] >>= n;
                renorm(&mut x[l], &mut wp, l);
            }
        }
        for l in 0..NL {
            if !live[l] {
                continue;
            }
            let c = &mut cur[l];
            let ch = c.ch as usize;
            let mut next = false;
            if c.k == 0 {
                kinds[0] += 1;
                eob_hist[ch][(sym[l] & 63) as usize] += 1;
            } else if sym[l] == ZRL {
                kinds[1] += 1;
            } else {
                kinds[2 + (sym[l] >> 5).min(3) as usize] += 1;
                cls[(sym[l] & 31) as usize] += 1;
            }
            if c.k == 0 {
                c.eob = sym[l] as u8;
                if ch == 0 {
                    c.luma = c.eob;
                }
                if c.eob == 0 {
                    next = true;
                } else {
                    c.k = 1;
                }
            } else if sym[l] == ZRL {
                let k = c.k as u32 + 4;
                if k > c.eob as u32 {
                    next = true;
                } else {
                    c.k = k as u8;
                }
            } else {
                let pos = c.k as u32 + (sym[l] >> 5);
                if pos >= c.eob as u32 {
                    next = true;
                } else {
                    c.k = (pos + 1) as u8;
                }
            }
            let _ = extra[l];
            if next {
                c.k = 0;
                c.ch += 1;
                if c.ch as usize == NCH {
                    c.ch = 0;
                    c.bx += NL as u32;
                    if c.bx as usize >= bw {
                        c.bx = l as u32;
                        c.r += 1;
                        if c.r as usize == nby {
                            c.done = true;
                        }
                    }
                }
            }
        }
        if wp > words.len {
            break;
        }
    }
    for w in lanes.iter_mut() {
        w.extend_from_slice(&[0; 8]);
    }
    LaneSplit {
        x0,
        words: lanes,
        active,
        bw,
        nby,
        tokens,
        eob_hist,
        kinds,
        cls,
    }
}

/// Share of fully-flat blocks (all `NCH` channel `eobs` zero): the
/// fraction of 8x8 blocks whose color output is a constant pixel — an
/// upper bound for flat-run color-skip candidates. Works for every
/// entropy mode (`eobs` is filled uniformly); not for column-grouped files,
/// whose records are per (stripe, group).
#[cfg(feature = "std")]
pub(crate) fn flat_share(data: &[u8]) -> Result<String, crate::Error> {
    let p = super::parse_lossy(data)?;
    if p.gcols != 0 {
        return Err(crate::Error::Unsupported(
            "flat share needs a file without column groups",
        ));
    }
    let (mut flat, mut total) = (0u64, 0u64);
    let mut per_ch = [0u64; NCH];
    for s in 0..p.stripes.len() {
        let (_, eobs, _, _) = decode_stripe_raw(&p, s, Group::ALL, false, Vec::new(), Vec::new())?;
        let nb = eobs.len().saturating_sub(1) / NCH;
        for b in 0..nb {
            let e = &eobs[b * NCH..b * NCH + NCH];
            flat += e.iter().all(|&v| v == 0) as u64;
            for ch in 0..NCH {
                per_ch[ch] += (e[ch] == 0) as u64;
            }
            total += 1;
        }
    }
    Ok(alloc::format!(
        "flat blocks {flat}/{total} ({:.1}%); per-ch zeros {per_ch:?}",
        flat as f64 * 100.0 / total.max(1) as f64
    ))
}

/// Per-channel histogram of the split-Huffman coded-group counts `eobs`
/// (0..=16 per block-channel), taken from the production decode grammar:
/// each stripe is decoded through `decode_stripe_raw`'s scalar path and
/// the `eobs` it produces are the values `split_unpack` consumes. Only
/// SplitHuf carries this data; other entropy modes are errors.
#[cfg(feature = "std")]
pub(crate) fn counts_histogram(data: &[u8]) -> Result<String, crate::Error> {
    let p = super::parse_lossy(data)?;
    if p.tables.mode != super::LossyEntropy::SplitHuf {
        return Err(crate::Error::Unsupported(
            "counts histogram requires split-Huffman",
        ));
    }
    let mut hist = [[0u64; 17]; NCH];
    for s in 0..p.stripes.len() {
        let (_, eobs, _, _) = decode_stripe_raw(&p, s, Group::ALL, false, Vec::new(), Vec::new())?;
        let nbc = eobs.len().saturating_sub(1) / NCH;
        for bc in 0..nbc {
            for ch in 0..NCH {
                let n = eobs[bc * NCH + ch] as usize;
                if n < hist[ch].len() {
                    hist[ch][n] += 1;
                }
            }
        }
    }
    let mut out = String::new();
    for (ch, h) in hist.iter().enumerate() {
        let n: u64 = h.iter().sum();
        out += &alloc::format!("ch{ch} (bcs {n}):");
        for (k, &v) in h.iter().enumerate() {
            out += &alloc::format!(" {k}:{v}");
        }
        out += "\n";
    }
    Ok(out)
}

/// Token mix and EOB distribution of a lossy file.
#[cfg(feature = "std")]
pub(crate) fn stats(data: &[u8]) -> String {
    let p = super::parse_lossy(data).unwrap();
    let mut eh = [[0u64; 64]; 3];
    let mut kinds = [0u64; 6];
    let mut cls = [0u64; 32];
    for s in 0..p.stripes.len() {
        let sp = split(&p, s);
        for c in 0..3 {
            for e in 0..64 {
                eh[c][e] += sp.eob_hist[c][e] as u64;
            }
        }
        for k in 0..6 {
            kinds[k] += sp.kinds[k] as u64;
        }
        for k in 0..32 {
            cls[k] += sp.cls[k] as u64;
        }
    }
    let tot: u64 = kinds.iter().sum();
    let mut out = alloc::format!(
        "tokens {tot}: EOB {:.1}% ZRL {:.1}% coef run0 {:.1}% run1 {:.1}% run2 {:.1}% run3 {:.1}%\n",
        kinds[0] as f64 * 100.0 / tot as f64,
        kinds[1] as f64 * 100.0 / tot as f64,
        kinds[2] as f64 * 100.0 / tot as f64,
        kinds[3] as f64 * 100.0 / tot as f64,
        kinds[4] as f64 * 100.0 / tot as f64,
        kinds[5] as f64 * 100.0 / tot as f64
    );
    let nc: u64 = cls.iter().sum();
    out += "  coef magnitude class share:";
    for (k, &v) in cls.iter().enumerate().take(10) {
        out += &alloc::format!(" {}:{:.1}%", k, v as f64 * 100.0 / nc as f64);
    }
    out += "\n";
    for c in 0..3 {
        let n: u64 = eh[c].iter().sum();
        let cum = |m: usize| eh[c][..=m].iter().sum::<u64>() as f64 * 100.0 / n as f64;
        let mean = eh[c]
            .iter()
            .enumerate()
            .map(|(e, &v)| e as f64 * v as f64)
            .sum::<f64>()
            / n as f64;
        out += &alloc::format!(
            "  ch{c} eob: =0 {:.1}%  <=2 {:.1}%  <=5 {:.1}%  <=9 {:.1}%  <=14 {:.1}%  <=27 {:.1}%  mean {:.1}\n",
            cum(0),
            cum(2),
            cum(5),
            cum(9),
            cum(14),
            cum(27),
            mean
        );
    }
    out
}

/// EOB-context class by luma EOB, and remaining-positions class by eob - k.
static EOBC: [u8; 65] = {
    let mut t = [0u8; 65];
    let mut i = 0;
    while i < 65 {
        t[i] = (32 - (i as u32).leading_zeros()) as u8;
        i += 1;
    }
    t
};
static REMC: [u8; 64] = {
    let mut t = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        let c = (31 - (i as u32 + 1).leading_zeros()) as usize;
        t[i] = if c < REM_CLASSES - 1 {
            c
        } else {
            REM_CLASSES - 1
        } as u8;
        i += 1;
    }
    t
};

#[derive(Clone, Copy)]
struct Lane {
    x: u32,
    wp: u32,
    /// Coefficient base of the current (block, channel): (bi * 3 + ch) * 64.
    qb: u32,
    bi: u32,
    bx: u32,
    ch: u32,
    k: u32,
    eob: u32,
    luma: u32,
    done: bool,
}

/// Decodes one token of lane `l` (inlined so `G` lanes interleave).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn token(
    l: &mut Lane,
    lane: u32,
    w: &[u16],
    cmap: &[u8; NRAW_PAD],
    dec: &[u32; MAX_TABLES << LOG_M],
    bw: u32,
    end: u32,
    q: &mut [i32],
    eobs: &mut [u8],
) {
    let raw = if l.k == 0 {
        l.ch as usize * 8 + EOBC[l.luma as usize & 63] as usize
    } else {
        EOB_CTX
            + (l.ch as usize * K_CLASSES + l.k as usize - 1) * REM_CLASSES
            + REMC[(l.eob - l.k) as usize & 63] as usize
    };
    let t = cmap[raw & (NRAW_PAD - 1)] as usize;
    let e = dec[((t << LOG_M) | (l.x & (M - 1)) as usize) & ((MAX_TABLES << LOG_M) - 1)];
    let mut x = ((e & 1023) + 1) * (l.x >> LOG_M) + ((e >> 10) & 1023);
    let mut wp = l.wp as usize;
    if x < RANS_L {
        x = (x << 16) | w[wp] as u32;
        wp += 1;
    }
    let sym = (e >> 20) & 127;
    let nb = e >> 27;
    let mut extra = 0;
    if nb > 0 {
        let n = nb.min(16);
        extra = x & crate::mask_of(n);
        x >>= n;
        if x < RANS_L {
            x = (x << 16) | w[wp] as u32;
            wp += 1;
        }
        if nb > 16 {
            let n = nb - 16;
            extra |= (x & crate::mask_of(n)) << 16;
            x >>= n;
            if x < RANS_L {
                x = (x << 16) | w[wp] as u32;
                wp += 1;
            }
        }
    }
    l.x = x;
    l.wp = wp.min(w.len() - 3) as u32;
    let next;
    if l.k == 0 {
        l.eob = sym;
        eobs[(l.qb / 64) as usize] = sym as u8;
        if l.ch == 0 {
            l.luma = sym;
        }
        next = sym == 0;
        l.k = 1;
    } else if sym == ZRL {
        l.k += 4;
        next = l.k > l.eob;
    } else {
        let pos = l.k + (sym >> 5);
        if pos <= l.eob {
            q[l.qb as usize + ZIGZAG[pos as usize & 63] as usize] = rl_value(sym & 31, extra);
        }
        next = pos >= l.eob;
        l.k = pos + 1;
    }
    if next {
        l.k = 0;
        l.ch += 1;
        l.qb += 64;
        if l.ch as usize == NCH {
            l.ch = 0;
            l.bx += NL as u32;
            l.bi += NL as u32;
            if l.bx >= bw {
                // First column of this lane in the next block row.
                l.bi = l.bi - l.bx + bw + lane;
                l.bx = lane;
                l.done = l.bi >= end;
            }
            l.qb = l.bi * NCH as u32 * 64;
        }
    }
}

/// Decodes a split stripe, `G` lanes interleaved per loop.
pub(crate) fn decode_lanes<const G: usize>(
    sp: &LaneSplit,
    cmap: &[u8; NRAW_PAD],
    dec: &[u32; MAX_TABLES << LOG_M],
    q: &mut [i32],
    eobs: &mut [u8],
) {
    let (bw, end) = (sp.bw as u32, (sp.nby * sp.bw) as u32);
    for g0 in (0..sp.active).step_by(G) {
        let mut ls: [Lane; G] = core::array::from_fn(|i| {
            let l = g0 + i;
            Lane {
                x: sp.x0[l.min(NL - 1)],
                wp: 0,
                qb: (l * NCH * 64) as u32,
                bi: l as u32,
                bx: l as u32,
                ch: 0,
                k: 0,
                eob: 0,
                luma: 0,
                done: l >= sp.active || end == 0,
            }
        });
        loop {
            let mut live = false;
            for (i, l) in ls.iter_mut().enumerate() {
                if !l.done {
                    live = true;
                    let lane = (g0 + i) as u32;
                    token(
                        l,
                        lane,
                        &sp.words[lane as usize],
                        cmap,
                        dec,
                        bw,
                        end,
                        q,
                        eobs,
                    );
                }
            }
            if !live {
                break;
            }
        }
    }
}

/// Times the production kernel against per-lane decoding (G = 1, 2, 4, 8)
/// over every stripe of a lossy file; checks they agree.
#[cfg(feature = "std")]
pub(crate) fn bench(data: &[u8], iters: usize) -> String {
    use std::time::Instant;
    let p = super::parse_lossy(data).unwrap();
    let ns = p.stripes.len();
    let splits: Vec<LaneSplit> = (0..ns).map(|s| split(&p, s)).collect();
    let tokens: usize = splits.iter().map(|sp| sp.tokens).sum();
    // Reference coefficients per stripe from the production kernel.
    let mut refs = Vec::new();
    for s in 0..ns {
        let (q, e, _, _) =
            decode_stripe_raw(&p, s, Group::ALL, true, Vec::new(), Vec::new()).unwrap();
        refs.push((q, e));
    }
    let mut q: Vec<i32> = Vec::new();
    let mut eobs: Vec<u8> = Vec::new();
    let mut check = |name: &str, f: &dyn Fn(usize, &mut Vec<i32>, &mut Vec<u8>)| -> f64 {
        let mut best = f64::MAX;
        for it in 0..iters {
            let mut t = 0.0;
            for s in 0..ns {
                q.clear();
                q.resize(refs[s].0.len(), 0);
                eobs.clear();
                eobs.resize(refs[s].1.len(), 0);
                let t0 = Instant::now();
                f(s, &mut q, &mut eobs);
                t += t0.elapsed().as_secs_f64();
                if it == 0 {
                    let n = q.len() - 64;
                    assert!(
                        q[..n] == refs[s].0[..n],
                        "{name}: coefficients differ in stripe {s}"
                    );
                    let n = eobs.len() - 1;
                    assert!(
                        eobs[..n] == refs[s].1[..n],
                        "{name}: EOBs differ in stripe {s}"
                    );
                }
            }
            best = best.min(t);
        }
        best
    };
    let mut out = String::new();
    let (cmap, dec) = (p.cmap(), p.dec());
    let sp = &splits;
    let mut res: Vec<(&str, f64)> = Vec::new();
    res.push((
        "kernel",
        check("kernel", &|s, q, e| {
            let (qq, ee) = (core::mem::take(q), core::mem::take(e));
            let (qq, ee, _, _) = decode_stripe_raw(&p, s, Group::ALL, true, qq, ee).unwrap();
            *q = qq;
            *e = ee;
        }),
    ));
    res.push((
        "lanes1",
        check("lanes1", &|s, q, e| {
            decode_lanes::<1>(&sp[s], cmap, dec, q, e)
        }),
    ));
    res.push((
        "lanes2",
        check("lanes2", &|s, q, e| {
            decode_lanes::<2>(&sp[s], cmap, dec, q, e)
        }),
    ));
    res.push((
        "lanes4",
        check("lanes4", &|s, q, e| {
            decode_lanes::<4>(&sp[s], cmap, dec, q, e)
        }),
    ));
    res.push((
        "lanes8",
        check("lanes8", &|s, q, e| {
            decode_lanes::<8>(&sp[s], cmap, dec, q, e)
        }),
    ));
    // Two stripes per call through the AVX-512 pair kernel.
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if let Some(t) = <archmage::X64V4Token as archmage::SimdToken>::summon() {
        let mut best = f64::MAX;
        let mut q2: [Vec<i32>; 2] = [Vec::new(), Vec::new()];
        let mut e2: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
        for it in 0..iters {
            let mut tt = 0.0;
            for s0 in (0..ns).step_by(2) {
                let pair = [s0, (s0 + 1).min(ns - 1)];
                for k in 0..2 {
                    q2[k].clear();
                    q2[k].resize(refs[pair[k]].0.len(), 0);
                    e2[k].clear();
                    e2[k].resize(refs[pair[k]].1.len(), 0);
                }
                let g = &p.g;
                let nby = pair.map(|s| {
                    let (a, b) = g.rows(s);
                    b - a
                });
                let ws = pair.map(|s| {
                    crate::rans::Words::from_bytes(p.stripes[s], g.active_lanes()).unwrap()
                });
                let [qa, qb] = &mut q2;
                let [ea, eb] = &mut e2;
                let t0 = Instant::now();
                let m = super::entropy_avx512::Maps {
                    cmap,
                    cmap32: p.cmap32(),
                    dec,
                };
                super::entropy_avx512::decode_stripes2(
                    t,
                    super::kernel_choice::choice().rl == super::kernel_choice::Rl::Avx512Hw,
                    [&ws[0].0, &ws[1].0],
                    [ws[0].1, ws[1].1],
                    &m,
                    g.bw,
                    nby,
                    g.active_lanes(),
                    0,
                    3,
                    [&mut qa[..], &mut qb[..]],
                    [&mut ea[..], &mut eb[..]],
                    None,
                );
                tt += t0.elapsed().as_secs_f64();
                if it == 0 {
                    for k in 0..2 {
                        let r = &refs[pair[k]];
                        let n = q2[k].len() - 64;
                        assert!(
                            q2[k][..n] == r.0[..n],
                            "pair: coefficients differ in stripe {}",
                            pair[k]
                        );
                        let n = e2[k].len() - 1;
                        assert!(
                            e2[k][..n] == r.1[..n],
                            "pair: EOBs differ in stripe {}",
                            pair[k]
                        );
                    }
                }
            }
            best = best.min(tt);
        }
        res.push(("pair512", best));
    }
    let px = p.info.width as f64 * p.info.height as f64;
    out += &alloc::format!("{tokens} tokens, {:.3} tokens/px\n", tokens as f64 / px);
    for (n, t) in res {
        out += &alloc::format!(
            "  {n:<7} {:>7.2} ms  {:>6.2} ns/token  {:>6.1} Mpx/s\n",
            t * 1e3,
            t * 1e9 / tokens as f64,
            px / t / 1e6
        );
    }
    out
}

/// Writes a lossy file's coded symbol streams for external entropy-coder
/// benchmarks (`<dir>/<tag>_tok.bin`: one byte per token in coded order per
/// stripe; split-Huffman also `<tag>_mag.bin`: one byte per coefficient
/// magnitude in coded groups, clamped to 255) and returns what imij spends
/// on them: tokens and their table cost (log2(M / f) under the file's own
/// context-modeled tables — code length for split-Huffman), extra / raw
/// bits.
#[cfg(feature = "std")]
pub(crate) fn dump(data: &[u8], dir: &str, tag: &str) -> String {
    use super::coef::{PK_GROUPS, ZIGZAG};
    let p = super::parse_lossy(data).unwrap();
    let g = &p.g;
    let bw = g.bw;
    let (mut toks, mut mags) = (Vec::<u8>::new(), Vec::<u8>::new());
    // Per token: its raw context (u16 LE); per magnitude: its group width
    // and position j in the group.
    let (mut tctx, mut mctx) = (Vec::<u8>::new(), Vec::<u8>::new());
    let (mut cost, mut extra, mut magbits, mut signbits) = (0f64, 0u64, 0u64, 0u64);
    let f = |ctx: usize, tok: u32| -> f64 {
        let t = p.tables.ctx_map[ctx] as usize;
        (crate::rans::M as f64 / p.tables.freq[t][tok as usize].max(1) as f64).log2()
    };
    let mode = p.tables.mode;
    for s in 0..p.stripes.len() {
        let (q, _, _, _) =
            decode_stripe_raw(&p, s, Group::ALL, false, Vec::new(), Vec::new()).unwrap();
        let (by0, by1) = g.rows(s);
        let nby = by1 - by0;
        let nbc = nby * bw * NCH;
        let nz: Vec<u64> = (0..nbc)
            .map(|bc| {
                (1..64).fold(0u64, |m, k| {
                    m | (((q[bc * 64 + ZIGZAG[k] as usize] != 0) as u64) << k)
                })
            })
            .collect();
        let sc = super::StripeCoefs {
            by0,
            nby,
            bw,
            q: q[..nbc * 64].to_vec(),
            nz,
            aq: Vec::new(),
            modes: Vec::new(),

            splits: Vec::new(),
        };
        match mode {
            super::LossyEntropy::RunLevel => {
                super::stripe_tokens(&sc, bw, Group::ALL, |_, ctx, tok, nb, _| {
                    toks.push(tok as u8);
                    tctx.extend_from_slice(&(ctx as u16).to_le_bytes());
                    cost += f(ctx, tok);
                    extra += nb as u64;
                });
            }
            _ => {
                // Split-Huffman: the count and width symbols in section
                // order, costed at the file's Huffman code lengths.
                let (counts, widths) = super::stripe_counts_widths(&sc, bw);
                let syms = super::huf::stripe_syms(&counts, &widths, nby * bw);
                let Some((tables, _, _)) = &p.huf else {
                    unreachable!("split-Huffman without its tables");
                };
                for (syms, map) in [
                    (&syms.luma, &tables.luma_map[..]),
                    (&syms.chroma, &tables.chroma_map[..]),
                    (&syms.wid, &tables.wid_map[..]),
                ] {
                    for &(ctx, sym) in syms {
                        toks.push(sym);
                        tctx.extend_from_slice(&(ctx as u16).to_le_bytes());
                        cost += tables.lens[map[ctx as usize] as usize][sym as usize] as f64;
                    }
                }
                for bc in 0..nbc {
                    let ac = sc.nz[bc] & !1;
                    let eob = if ac == 0 {
                        0
                    } else {
                        63 - ac.leading_zeros() as usize
                    };
                    for gi in 0..eob.div_ceil(4) {
                        let len = if gi == PK_GROUPS - 1 { 3 } else { 4 };
                        let vals: Vec<i32> = (0..len)
                            .map(|j| sc.q[bc * 64 + ZIGZAG[1 + 4 * gi + j] as usize])
                            .collect();
                        let m = vals.iter().map(|v| v.unsigned_abs()).max().unwrap();
                        let w = 32 - m.leading_zeros();
                        for (j, v) in vals.into_iter().enumerate() {
                            mctx.push((w as u8) << 2 | j as u8);
                            mags.push(v.unsigned_abs().min(255) as u8);
                            magbits += w as u64;
                            signbits += (v != 0) as u64;
                        }
                    }
                }
            }
        }
    }
    std::fs::write(alloc::format!("{dir}/{tag}_tok.bin"), &toks).unwrap();
    std::fs::write(alloc::format!("{dir}/{tag}_tok.ctx"), &tctx).unwrap();
    if !mctx.is_empty() {
        std::fs::write(alloc::format!("{dir}/{tag}_mag.ctx"), &mctx).unwrap();
    }
    if !mags.is_empty() {
        std::fs::write(alloc::format!("{dir}/{tag}_mag.bin"), &mags).unwrap();
    }
    alloc::format!(
        "{tag}: {} tokens, imij rANS cost {:.0} B ({:.3} bits/token); extra bits {} B; {} magnitudes, raw {} B, signs {} B\n",
        toks.len(),
        cost / 8.0,
        cost / toks.len().max(1) as f64,
        extra / 8,
        mags.len(),
        magbits / 8,
        signbits / 8
    )
}
