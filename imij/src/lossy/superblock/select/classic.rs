//! Classic greedy superblock and 4x4 small-transform selector (used when
//! adaptive RDO map DP, trellis-in-selection, learned pilot tables, and
//! open-loop lambda AQ are all inactive).

use super::coder::{Coder, Refs, nonzero, ring_unit, unit_bits_m};
use super::transform::*;
use super::{
    DC_KAPPA, FLAT_A0, FlagCost, Group2, Opts, RING, RowBuffers, SYM_MERGE_BITS, SYM_NONE_BITS,
    Selector, Solo, Written, ac_of, prev_refs, prune_units, score_merged_ringing_and_rate,
    write_merged, write_split,
};
use crate::lossy::superblock::*;
use crate::lossy::trellis::LossyTrellis;
use archmage::prelude::*;

impl Selector {
    /// Classic greedy partition selector (without adaptive RDO map DP,
    /// trellis-in-selection, learned pilot tables, or open-loop lambda AQ).
    pub(super) fn stripe_classic(
        &self,
        mut sc: StripeCoefs,
        rows: RowPlanes,
        stop: &dyn how_far::Stop,
    ) -> Result<StripeCoefs, crate::Error> {
        let (pw, group_bw, round, opts) = (self.pw, self.group_bw, self.round, self.opts);
        let (coder, enc, lam) = (&self.coder, &self.enc, &self.lam);
        let cm = enc.b.m8;
        let (bw, nby) = (sc.bw, sc.nby);
        sc.modes = crate::lossy::adaptive_rdo::try_vec(nby * bw, 0u8)?;
        if opts.small.is_some() {
            sc.splits = crate::lossy::adaptive_rdo::try_vec(nby * bw, 0u8)?;
        }
        let mut row_bufs = RowBuffers::new(pw, bw)?;
        let mut work = Work::new();
        let mut flags = FlagCost::new();
        let mut written = Vec::new();
        for r in (0..nby).step_by(2) {
            stop.check().map_err(|_| crate::Error::Cancelled)?;
            let rr = if r + 1 < nby { 2 } else { 1 };
            row_bufs.load_pair(rows, sc.by0 + r, rr, pw, bw, &cm);
            let mut bx = 0;
            while bx < bw {
                // Groups are aligned within each column group.
                let gs = bx.checked_div(group_bw).map_or(0, |g| g * group_bw);
                let ge = if group_bw == 0 {
                    bw
                } else {
                    (gs + group_bw).min(bw)
                };
                let cols = if bx + 1 < ge { 2 } else { 1 };
                if rr * cols > 1 || opts.small.is_some() {
                    let g = Group2 {
                        r,
                        rows: rr,
                        bx,
                        cols,
                    };
                    decide(
                        &mut sc,
                        enc,
                        &row_bufs.planes,
                        &row_bufs.ext,
                        &row_bufs.coefs,
                        pw,
                        &g,
                        round,
                        coder,
                        lam,
                        opts,
                        &mut work,
                        &mut flags,
                        &mut written,
                    )?;
                }
                bx += cols;
            }
        }
        if opts.small.is_some_and(|f| f.is_none()) {
            prune_units(&mut sc, group_bw, &written);
        }
        Ok(sc)
    }
}

#[allow(clippy::too_many_arguments)]
fn decide(
    sc: &mut StripeCoefs,
    enc: &Enc,
    planes: &[Vec<f32>; 2],
    ext: &[Vec<(u64, bool)>; 2],
    coefs: &[Vec<f32>; 2],
    pw: usize,
    g: &Group2,
    round: f32,
    coder: &Coder,
    lam: &LossyTrellis,
    opts: Opts,
    work: &mut Work,
    flags: &mut FlagCost,
    written: &mut Vec<Written>,
) -> Result<(), crate::Error> {
    let bw = sc.bw;
    let b_of = |py: usize, px: usize| (g.r + py) * bw + g.bx + px;
    // Parents that may merge or split: not kept or exact (code 0 here),
    // not an all-white / all-black block (their DCs round toward the
    // clamp).
    let code = |b: usize| sc.aq.get(b).copied().unwrap_or(64);
    let mut ok = [[false; 2]; 2];
    for py in 0..g.rows {
        for px in 0..g.cols {
            let extreme = ext[py]
                .get(g.bx + px)
                .is_some_and(|&(white, black)| white == u64::MAX || black);
            ok[py][px] = code(b_of(py, px)) != 0 && !extreme;
        }
    }
    let all_ok = (0..g.rows).all(|py| (0..g.cols).all(|px| ok[py][px]));
    let merges = opts.merges && all_ok && g.rows * g.cols > 1;
    if !merges && opts.small.is_none() {
        return Ok(());
    }
    // 4x4 candidates per parent, bit `1 << ch`: a plane whose AC energy is
    // below the smallest 4x4 AC threshold quantizes every 4x4 AC slot to
    // zero (each |c| is at most the energy's root), so its 4x4 coding has
    // no AC and is no candidate.
    let mut cand = [[0u8; 2]; 2];
    if opts.small.is_some() {
        for py in 0..g.rows {
            for px in 0..g.cols {
                if !ok[py][px] {
                    continue;
                }
                let aq = code(b_of(py, px)) as f32 / 64.0;
                for ch in 0..NCH {
                    let c = &coefs[py][(ch * bw + g.bx + px) * 64..][..64];
                    let ac_energy: f32 = c[1..].iter().map(|&v| v * v).sum();
                    let thr = enc.min4[ch] * aq * (1.0 - round.abs());
                    cand[py][px] |= ((ac_energy >= thr * thr) as u8) << ch;
                }
            }
        }
    }

    if !merges && cand.iter().flatten().all(|&m| m == 0) {
        // Nothing to choose: every parent keeps pass 1's 8x8 coding.
        for py in 0..g.rows {
            for px in 0..g.cols {
                if ok[py][px] {
                    flags.add(0, ac_of(sc, b_of(py, px)));
                }
            }
        }
        return Ok(());
    }
    // Per plane: zenjpeg's trellis lambda on the group's mean AC energy
    // (libjpeg units), and each parent's flatness weight.
    let mut w = [0.0f64; NCH];
    let mut wt = [[[0.0f32; 2]; 2]; NCH];
    for ch in 0..NCH {
        let mut sum = 0.0f64;
        for py in 0..g.rows {
            for px in 0..g.cols {
                let c = &coefs[py][(ch * bw + g.bx + px) * 64..][..64];
                let e: f64 = c[1..].iter().map(|&v| (v as f64) * (v as f64)).sum();
                sum += e;
                let aq = code(b_of(py, px)) as f32 / 64.0;
                let d = enc.steps8[ch][1] * aq;
                let ms = (e / 63.0) as f32 / (d * d);
                wt[ch][py][px] = FLAT_A0 / (FLAT_A0 + ms);
            }
        }
        let n = (g.rows * g.cols) as f64;
        w[ch] = 64.0 * lam.lambda(sum / n * 64.0 / 63.0);
    }

    // Parents coded alone: as pass 1 quantized them, or per plane the
    // cheaper of that and its 4x4 coding.
    let mut solo = [[Solo {
        j: 0.0,
        split: 0,
        q4: [[0; 64]; NCH],
        gain: [0.0; NCH],
    }; 2]; 2];
    for py in 0..g.rows {
        for px in 0..g.cols {
            // Without merges only the split choice matters (not the cost).
            if !merges && cand[py][px] == 0 {
                continue;
            }
            let small = opts.small.filter(|_| ok[py][px]);
            solo[py][px] = solo_parent(
                sc,
                enc,
                planes,
                coefs,
                pw,
                g,
                (py, px),
                &w,
                &wt,
                round,
                coder,
                small.map(|f| (f, cand[py][px])),
                &opts,
                flags,
                &prev_refs,
            );
        }
    }
    let mut best = (f64::MAX, 0usize);
    let Work { sp, cands, err } = work;
    let mut cand_ix = [[None::<usize>; 4]; 3];
    if merges {
        // Merged candidates, from every plane's candidate spectra.
        enc.spectra(planes, pw, g, sp);
        let mut n = 0;
        for blocks in BLOCKS.iter().skip(1) {
            for &(s, py0, px0) in *blocks {
                if py0 + s.ph() > g.rows || px0 + s.pw() > g.cols || opts.shapes >> s as u8 & 1 == 0
                {
                    continue;
                }
                if cand_ix[s as usize][py0 * 2 + px0].is_none() {
                    cand_ix[s as usize][py0 * 2 + px0] = Some(n);
                    let c = &mut cands[n];
                    let pr = |b: usize| prev_refs(sc, b);
                    merged(
                        sc,
                        enc,
                        sp,
                        g,
                        (s, py0, px0),
                        &w,
                        &wt,
                        round,
                        coder,
                        &pr,
                        c,
                        err,
                    );
                    n += 1;
                }
            }
        }
        // The cheapest partition (ties keep the earlier symbol, no merge
        // first), with the symbol costs of a map without statistics.
        let has = [true, g.cols == 2, g.rows == 2, g.cols == 2 && g.rows == 2];
        for (sym, roles) in PARTITIONS.iter().enumerate() {
            if (0..4).any(|i| roles[i] != 0 && !has[i]) {
                continue;
            }
            let mut j = if sym == 0 {
                SYM_NONE_BITS
            } else {
                SYM_MERGE_BITS
            };
            let mut covered = [false; 4];
            let mut allowed = true;
            for &(s, py0, px0) in BLOCKS[sym] {
                let Some(ci) = cand_ix[s as usize][py0 * 2 + px0] else {
                    allowed = false;
                    break;
                };
                j += cands[ci].j + DC_KAPPA * (s.ph() * s.pw()) as f64;
                for py in 0..s.ph() {
                    for px in 0..s.pw() {
                        covered[(py0 + py) * 2 + px0 + px] = true;
                    }
                }
            }
            if !allowed {
                continue;
            }
            for py in 0..g.rows {
                for px in 0..g.cols {
                    if !covered[py * 2 + px] {
                        j += solo[py][px].j;
                    }
                }
            }
            if j < best.0 {
                best = (j, sym);
            }
        }
    } else {
        best.1 = 0;
    }
    let mut covered = [false; 4];
    for &(s, py0, px0) in BLOCKS[best.1] {
        let ci = cand_ix[s as usize][py0 * 2 + px0].expect("candidate");
        write_merged(sc, s, g.r + py0, g.bx + px0, &cands[ci]);
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                covered[(py0 + py) * 2 + px0 + px] = true;
            }
        }
    }
    if opts.small.is_some() {
        for py in 0..g.rows {
            for px in 0..g.cols {
                if covered[py * 2 + px] || !ok[py][px] {
                    continue;
                }
                let so = &solo[py][px];
                let b = b_of(py, px);
                for ch in 0..NCH {
                    if so.split >> ch & 1 != 0 {
                        crate::lossy::adaptive_rdo::try_push(
                            written,
                            Written {
                                b,
                                ch,
                                q8: sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap(),
                                nz8: sc.nz[b * NCH + ch],
                                gain: so.gain[ch],
                            },
                        )?;
                    }
                }
                write_split(sc, b, so);
                flags.add(so.split, ac_of(sc, b));
            }
        }
    }
    Ok(())
}

/// The cost of parent (`py`, `px`) of group `g` coded alone and, with
/// `small` (small transforms: `(force, cand)`, `force = Some(m)` forcing the
/// planes of `m`, `cand` the planes with a 4x4 candidate), its cheaper
/// coding per plane: luma first against the lane's previous luma
/// unit, each chroma plane against the luma option it follows; luma's
/// option is the cheaper including the chroma planes it makes cheaper or
/// dearer.
#[allow(clippy::too_many_arguments)]
fn solo_parent(
    sc: &StripeCoefs,
    enc: &Enc,
    planes: &[Vec<f32>; 2],
    coefs: &[Vec<f32>; 2],
    pw: usize,
    g: &Group2,
    (py, px): (usize, usize),
    w: &[f64; NCH],
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    small: Option<(Option<u8>, u8)>,
    opts: &Opts,
    flags: &FlagCost,
    prev_refs: &dyn Fn(&StripeCoefs, usize) -> Refs,
) -> Solo {
    let bw = sc.bw;
    let b = (g.r + py) * bw + g.bx + px;
    let aq = sc.aq.get(b).copied().unwrap_or(64) as f32 / 64.0;
    let r0 = prev_refs(sc, b);
    // Per plane, 8x8 and 4x4: the units, their nonzero masks, the
    // distortion term and the split choice's extra term (the distortion
    // again times kappa - 1 plus the inner-edge term, both weighted).
    let q8: [&[i32; 64]; NCH] =
        core::array::from_fn(|ch| sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap());
    let m8 = q8.map(nonzero);
    let (mut d8, mut x8) = ([0.0f64; NCH], [0.0f64; NCH]);
    let mut q4 = [[0i32; 64]; NCH];
    let (mut m4, mut d4, mut x4) = ([0u64; NCH], [0.0f64; NCH], [0.0f64; NCH]);
    let kx = (opts.kappa - 1.0) as f64;
    for ch in 0..NCH {
        let c: &[f32; 64] = coefs[py][(ch * bw + g.bx + px) * 64..][..64]
            .try_into()
            .unwrap();
        let st = &enc.steps8[ch];
        let (d, en) = incant!(
            terms_impl(c, q8[ch], (&enc.inv8[ch], st), aq),
            [v3, neon, wasm128, scalar]
        );
        let ring = RING * ring_unit(st[1] * aq, wt[ch][py][px], en);
        (d8[ch], x8[ch]) = (w[ch] * (d + ring), w[ch] * kx * (d + ring));
        if small.is_some_and(|(_, cand)| cand >> ch & 1 != 0) {
            let mut pix = [0.0f32; 64];
            for (y, row) in pix.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                row.copy_from_slice(&planes[py][ch * 8 * pw + y * pw + (g.bx + px) * 8..][..8]);
            }
            let cf = (opts.forward)(&pix);
            let (d, en) = incant!(
                cand4_impl(
                    &cf,
                    (&enc.inv4[ch], &enc.steps4[ch]),
                    (aq, round),
                    &mut q4[ch]
                ),
                [v3, neon, wasm128, scalar]
            );
            // The DC is the parent's: pass 1's value (the DC image does not
            // depend on the split).
            q4[ch][0] = q8[ch][0];
            m4[ch] = nonzero(&q4[ch]);
            let ring = RING * ring_unit(st[1] * aq, wt[ch][py][px], en);
            (d4[ch], x4[ch]) = (w[ch] * (d + ring), w[ch] * kx * (d + ring));
            if opts.edge > 0.0 {
                let first = (st[1] * aq) as f64;
                let k = w[ch] * (opts.kappa * opts.edge) as f64 / (first * first);
                x8[ch] += k * inner_edge(&pix, &recon(q8[ch], &enc.dq8[ch], aq, false));
                x4[ch] += k * inner_edge(&pix, &recon(&q4[ch], &enc.dq4[ch], aq, true));
            }
        }
    }
    // Plane `ch`'s units coded 8x8 (`s` 0) or 4x4 (1), and their bits:
    // luma against the lane's previous luma unit, chroma against luma
    // option `ls` (memoized; every option prices the same units).
    let unit = |ch: usize, s: usize| -> (&[i32; 64], u64) {
        if s == 0 {
            (q8[ch], m8[ch])
        } else {
            (&q4[ch], m4[ch])
        }
    };
    let lref = [Refs::of_mask(m8[0]), Refs::of_mask(m4[0])];
    let mut lmemo = [None::<f64>; 2];
    let mut lbits = |s: usize| {
        *lmemo[s].get_or_insert_with(|| {
            let (q, m) = unit(0, s);
            unit_bits_m(coder, 0, q, m, r0)
        })
    };
    let mut cmemo = [[[None::<f64>; 2]; 2]; NCH];
    let mut cbits = |ch: usize, ls: usize, s: usize| {
        *cmemo[ch][ls][s].get_or_insert_with(|| {
            let (q, m) = unit(ch, s);
            unit_bits_m(coder, ch, q, m, lref[ls])
        })
    };
    let Some((force, _)) = small else {
        // 8x8 only.
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        return Solo {
            j,
            split: 0,
            q4: [[0; 64]; NCH],
            gain: [0.0; NCH],
        };
    };
    // The choice minimizes J plus the extra terms; the J it reports (for
    // the merge decision) has none of them.
    // A plane carries a split flag only when its coding has AC (SPEC
    // 2.11.1); a 4x4 coding without AC is the 8x8 flat block, not a split.
    let ac4 = m4.map(|m| m != 0);
    let mut best = (f64::MAX, 0u8, 0.0f64);
    for lsplit in 0..2u8 {
        if force.is_some_and(|m| (m & 1) != lsplit) || (lsplit == 1 && !ac4[0]) {
            continue;
        }
        let ls = lsplit as usize;
        let (lc, lx, lm) = if ls == 0 {
            (d8[0], x8[0], m8[0])
        } else {
            (d4[0], x4[0], m4[0])
        };
        let fl = if lm != 0 { flags.bits(0, 0, ls) } else { 0.0 };
        let mut j = lc + lbits(ls) + fl;
        let mut x = lx;
        let mut sp = lsplit;
        for ch in 1..NCH {
            let mut bj = (f64::MAX, 0u8, 0.0f64);
            for csplit in 0..2u8 {
                if force.is_some_and(|m| (m >> ch & 1) != csplit) || (csplit == 1 && !ac4[ch]) {
                    continue;
                }
                let cs = csplit as usize;
                let (cc, cx, cm) = if cs == 0 {
                    (d8[ch], x8[ch], m8[ch])
                } else {
                    (d4[ch], x4[ch], m4[ch])
                };
                let fl = if cm != 0 { flags.bits(ch, ls, cs) } else { 0.0 };
                let jj = cc + cbits(ch, ls, cs) + fl;
                if jj + cx < bj.0 + bj.2 {
                    bj = (jj, csplit, cx);
                }
            }
            if bj.0 == f64::MAX {
                // Forced 4x4 without AC: the 8x8 coding.
                bj = (d8[ch] + cbits(ch, ls, 0), 0, 0.0);
            }
            j += bj.0;
            x += bj.2;
            sp |= bj.1 << ch;
        }
        if j + x < best.0 + best.2 {
            best = (j, sp, x);
        }
    }
    if best.0 == f64::MAX {
        // Forced luma 4x4 without AC: the 8x8 codings.
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        best = (j, 0, 0.0);
    }
    // Gains of the chosen 4x4 planes (flags excluded): luma against the
    // lane's previous luma, chroma against the chosen luma unit.
    let mut out = [[0i32; 64]; NCH];
    let mut gain = [0.0f64; NCH];
    let ls = (best.1 & 1) as usize;
    for ch in 0..NCH {
        if best.1 >> ch & 1 != 0 {
            out[ch] = q4[ch];
            let (b8, b4) = if ch == 0 {
                (lbits(0), lbits(1))
            } else {
                (cbits(ch, ls, 0), cbits(ch, ls, 1))
            };
            gain[ch] = (d8[ch] + x8[ch] + b8) - (d4[ch] + x4[ch] + b4);
        }
    }
    Solo {
        j: best.0,
        split: best.1,
        q4: out,
        gain,
    }
}

/// Quantizes and prices the merged block of shape `s` at parent (`py0`,
/// `px0`) of group `g`.
#[allow(clippy::too_many_arguments)]
fn merged(
    sc: &StripeCoefs,
    enc: &Enc,
    sp: &[Spectra; NCH],
    g: &Group2,
    (s, py0, px0): (Shape, usize, usize),
    w: &[f64; NCH],
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    prev_refs: &dyn Fn(usize) -> Refs,
    out: &mut MCand,
    spec_err: &mut [[f32; 256]; NCH],
) {
    let bw = sc.bw;
    let b_of = |py: usize, px: usize| (g.r + py0 + py) * bw + g.bx + px0 + px;
    // AQ: the finest code among the parents.
    let aqc = (0..s.ph())
        .flat_map(|py| (0..s.pw()).map(move |px| (py, px)))
        .map(|(py, px)| sc.aq.get(b_of(py, px)).copied().unwrap_or(64))
        .min()
        .unwrap();
    let aq = aqc as f32 / 64.0;
    // Every unit slot but the DCs is written below; the DCs after it.
    out.shape = s;
    out.j = 0.0;
    out.aq = aqc;
    let mut qs = [0i32; 256];
    for ch in 0..NCH {
        let c = sp[ch].get(s, py0, px0);
        let si = s as usize;
        let tabs = (&enc.inv[ch][si], &enc.steps[ch][si], &enc.sc[si]);
        // The error goes through the kernel, so it is pre-scaled.
        let d = incant!(
            quant_impl(
                (s.h(), s.w() / 8),
                c,
                tabs,
                (aq, round),
                &mut qs,
                &mut spec_err[ch]
            ),
            [v3, neon, wasm128, scalar]
        );
        let gather = &enc.gather[si];
        for (p, unit) in out.units[ch].iter_mut().enumerate().take(s.ph() * s.pw()) {
            for (z, v) in unit.iter_mut().enumerate() {
                *v = qs[gather[p * 64 + z] as usize];
            }
        }
        out.j += w[ch] * d as f64;
        let dd = llf_to_dcs(s, [[c[0], c[1]], [c[16], c[17]]]);
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                // Nearest, half away from zero, within the coefficient cap.
                let v = dd[py][px] / enc.steps8[ch][0];
                let r = crate::mathx::round_pos(v.abs() as f64).min(4_194_303.0) as i32;
                out.dcs[ch][py][px] = if v < 0.0 { -r } else { r };
            }
        }
    }
    score_merged_ringing_and_rate(
        enc,
        s,
        (py0, px0),
        aq,
        w,
        wt,
        coder,
        &b_of,
        prev_refs,
        out,
        spec_err,
    );
}
