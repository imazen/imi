//! Surrounding-block adaptive RDO partition selector: trellis-priced solo and
//! merged candidates, exact 8-state row-strip Viterbi DP, unit-level V4
//! partition-map pruning, and open-loop lambda modulation.

use super::coder::{Coder, Refs, nonzero, ring_unit, unit_bits_m};
use super::transform::*;
use super::{
    DC_KAPPA, FLAT_A0, FlagCost, Group2, Opts, RING, RowBuffers, Selector, Solo, Written, ac_of,
    prev_refs, prune_units, score_merged_ringing_and_rate, write_merged, write_split,
};
use crate::lossy::superblock::*;
use crate::lossy::trellis::LossyTrellis;
use archmage::prelude::*;

impl Selector {
    /// Surrounding-block adaptive RDO stripe selector: trellis-priced solo and
    /// merged candidates, exact 8-state row-strip Viterbi DP over the 9-context
    /// partition-map model, exact unit-level V4 partition-map merge pruning,
    /// empirical pilot-pass map/flag tables, and unified open-loop lambda AQ.
    pub(super) fn stripe_rdo(
        &self,
        mut sc: StripeCoefs,
        rows: RowPlanes,
        lambda_map: &[f32],
        lambda_by0: usize,
        stop: &dyn how_far::Stop,
    ) -> Result<StripeCoefs, crate::Error> {
        use crate::lossy::adaptive_rdo::{
            MapBitTable, solve_row_strip_dp, sym_class, try_push, try_vec, try_vec_cap,
        };
        let (pw, group_bw, round, opts) = (self.pw, self.group_bw, self.round, self.opts);
        let rdo = opts.rdo;
        let (coder, enc, lam) = (&self.coder, &self.enc, &self.lam);
        let cm = enc.b.m8;
        let (bw, nby) = (sc.bw, sc.nby);
        let snb = nby.checked_mul(bw).ok_or(crate::Error::TooLarge)?;
        sc.modes = try_vec(snb, 0u8)?;
        if opts.small.is_some() {
            sc.splits = try_vec(snb, 0u8)?;
        }
        let map_bits = if let Some(m) = self.learned_map {
            m
        } else if rdo.map_dp {
            MapBitTable::prior()
        } else {
            MapBitTable::flat()
        };
        let mut flags = match self.learned_flags {
            Some(lf) => FlagCost { n: lf.n },
            None => FlagCost::new(),
        };
        let mut row_bufs = RowBuffers::new(pw, bw)?;
        let mut work = Work::new();
        let mut tcands: alloc::boxed::Box<crate::lossy::trellis::Cands> =
            alloc::boxed::Box::default();
        let mut written = Vec::new();

        let gw = if group_bw == 0 { bw } else { group_bw };
        let n_cg = bw.div_ceil(gw);
        let mut prev_sym_cg: Vec<Vec<u8>> = try_vec_cap(n_cg)?;
        for ci in 0..n_cg {
            let gs = ci * gw;
            let ge = (gs + gw).min(bw);
            prev_sym_cg.push(try_vec((ge - gs).div_ceil(2), 0u8)?);
        }

        // Unit-level merge pruning: record exact partition symbols, pre-merge
        // solo backups, and merge gain per column group so marginal merges that
        // do not pay for the actual V4 partition map bytes are reverted cleanly.
        struct MergedBackup {
            b: usize,
            q8: [[i32; 64]; NCH],
            nz8: [u64; NCH],
            aq8: Option<u8>,
            ok: bool,
            solo: Solo,
        }
        let grows = nby.div_ceil(2);
        let mut cg_syms: Vec<Vec<u8>> = try_vec_cap(n_cg)?;
        for ci in 0..n_cg {
            let gs = ci * gw;
            let ge = (gs + gw).min(bw);
            let sym_len = grows
                .checked_mul((ge - gs).div_ceil(2))
                .ok_or(crate::Error::TooLarge)?;
            cg_syms.push(try_vec(sym_len, 0u8)?);
        }
        let mut cg_merge_gain: Vec<f64> = try_vec(n_cg, 0.0)?;
        let mut cg_backups: Vec<Vec<MergedBackup>> = try_vec_cap(n_cg)?;
        for _ in 0..n_cg {
            cg_backups.push(Vec::new());
        }
        let mut has_prev_row = false;

        struct GroupEval {
            bx: usize,
            cols: usize,
            ok: [[bool; 2]; 2],
            solo: [[Solo; 2]; 2],
            cands: [MCand; 5],
            cand_ix: [[Option<usize>; 4]; 3],
        }

        let dummy_cand = MCand {
            shape: Shape::S16x16,
            j: 0.0,
            units: [[[0; 64]; 4]; NCH],
            dcs: [[[0; 2]; 2]; NCH],
            aq: 64,
        };
        let dummy_solo = Solo {
            j: 0.0,
            split: 0,
            q4: [[0; 64]; NCH],
            gain: [0.0; NCH],
        };

        for r in (0..nby).step_by(2) {
            stop.check().map_err(|_| crate::Error::Cancelled)?;
            let rr = if r + 1 < nby { 2 } else { 1 };
            row_bufs.load_pair(rows, sc.by0 + r, rr, pw, bw, &cm);
            let RowBuffers { planes, ext, coefs } = &row_bufs;

            for ci in 0..n_cg {
                let gs = ci * gw;
                let ge = (gs + gw).min(bw);
                let ng = (ge - gs).div_ceil(2);
                let mut evals: Vec<GroupEval> = try_vec_cap(ng)?;
                let mut unary: Vec<[f64; 8]> = try_vec_cap(ng)?;
                let mut up_class: Vec<u8> = try_vec_cap(ng)?;

                for gc in 0..ng {
                    stop.check().map_err(|_| crate::Error::Cancelled)?;
                    let bx = gs + gc * 2;
                    let cols = (ge - bx).min(2);
                    let g = Group2 {
                        r,
                        rows: rr,
                        bx,
                        cols,
                    };
                    let b_of = |py: usize, px: usize| (g.r + py) * bw + g.bx + px;
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

                    let mut w = [0.0f64; NCH];
                    let mut group_norm = [0.0f64; NCH];
                    let mut wt = [[[0.0f32; 2]; 2]; NCH];
                    let mut lmul_max = 0.0f64;
                    if !lambda_map.is_empty() {
                        for py in 0..g.rows {
                            let rel_by = (sc.by0 + g.r + py).saturating_sub(lambda_by0);
                            for px in 0..g.cols {
                                let gi = rel_by * bw + g.bx + px;
                                let m = lambda_map.get(gi).copied().unwrap_or(1.0) as f64;
                                if m > lmul_max {
                                    lmul_max = m;
                                }
                            }
                        }
                    }
                    let lmul_g = if lmul_max > 0.0 { lmul_max } else { 1.0 };
                    let n_par = (g.rows * g.cols) as f64;
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
                        group_norm[ch] = sum / n_par;
                        w[ch] = 64.0 * lam.lambda(group_norm[ch] * 64.0 / 63.0) * lmul_g;
                    }

                    let mut solo = [[dummy_solo; 2]; 2];
                    for py in 0..g.rows {
                        for px in 0..g.cols {
                            let small = opts.small.filter(|_| ok[py][px]);
                            solo[py][px] = solo_parent_rdo(
                                &sc,
                                enc,
                                planes,
                                coefs,
                                pw,
                                &g,
                                (py, px),
                                &w,
                                &group_norm,
                                lmul_g,
                                &wt,
                                round,
                                coder,
                                lam,
                                small.map(|f| (f, cand[py][px])),
                                &opts,
                                &flags,
                                &prev_refs,
                                &mut tcands,
                            );
                        }
                    }

                    let Work { sp, cands, err } = &mut *work;
                    let mut cand_ix = [[None::<usize>; 4]; 3];
                    if merges {
                        enc.spectra(planes, pw, &g, sp);
                        let mut n = 0;
                        for blocks in BLOCKS.iter().skip(1) {
                            for &(s, py0, px0) in *blocks {
                                if py0 + s.ph() > g.rows
                                    || px0 + s.pw() > g.cols
                                    || opts.shapes >> s as u8 & 1 == 0
                                {
                                    continue;
                                }
                                if cand_ix[s as usize][py0 * 2 + px0].is_none() {
                                    cand_ix[s as usize][py0 * 2 + px0] = Some(n);
                                    let c = &mut cands[n];
                                    let pr = |b: usize| prev_refs(&sc, b);
                                    merged_rdo(
                                        &sc,
                                        enc,
                                        sp,
                                        &g,
                                        (s, py0, px0),
                                        &w,
                                        &group_norm,
                                        lmul_g,
                                        &wt,
                                        round,
                                        coder,
                                        lam,
                                        &opts,
                                        &pr,
                                        &mut tcands,
                                        c,
                                        err,
                                    );
                                    n += 1;
                                }
                            }
                        }
                    }

                    let mut u_row = [f64::INFINITY; 8];
                    let has = [true, g.cols == 2, g.rows == 2, g.cols == 2 && g.rows == 2];

                    for (sym, roles) in PARTITIONS.iter().enumerate() {
                        if sym != 0 && !merges {
                            continue;
                        }
                        if (0..4).any(|i| roles[i] != 0 && !has[i]) {
                            continue;
                        }
                        let mut j = 0.0f64;
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
                        u_row[sym] = j;
                    }

                    unary.push(u_row);
                    up_class.push(if has_prev_row {
                        sym_class(prev_sym_cg[ci][gc]) as u8
                    } else {
                        0
                    });
                    let mut saved_cands: [MCand; 5] = core::array::from_fn(|_| dummy_cand.clone());
                    if merges {
                        saved_cands.clone_from_slice(cands);
                    }
                    evals.push(GroupEval {
                        bx,
                        cols,
                        ok,
                        solo,
                        cands: saved_cands,
                        cand_ix,
                    });
                }

                // Exact 8-state row-strip Viterbi DP across the column group's row.
                // Exclude a trailing 1x1 corner (odd unit rows AND odd unit cols),
                // which `superblock::map_groups` omits and emits zero map bits for.
                let trailing_1x1 = rr == 1 && !(ge - gs).is_multiple_of(2);
                let dp_ng = if trailing_1x1 { ng - 1 } else { ng };
                let mut path = if dp_ng > 0 {
                    solve_row_strip_dp(
                        &unary[..dp_ng],
                        &up_class[..dp_ng],
                        &map_bits,
                        |_, _, _| 0.0,
                    )?
                    .1
                } else {
                    Vec::new()
                };
                if trailing_1x1 {
                    try_push(&mut path, 0)?;
                }

                // Commit winning partition symbols to `sc`.
                let gr = r / 2;
                let gcols_cg = (ge - gs).div_ceil(2);
                for gc in 0..ng {
                    let sym = path[gc] as usize;
                    let ev = &evals[gc];
                    let sym_out = if rr == 1 && ev.cols == 1 {
                        0
                    } else {
                        sym as u8
                    };
                    cg_syms[ci][gr * gcols_cg + gc] = sym_out;
                    if sym != 0 {
                        cg_merge_gain[ci] += unary[gc][0] - unary[gc][sym];
                    }
                    let mut covered = [false; 4];
                    for &(s, py0, px0) in BLOCKS[sym] {
                        for py in 0..s.ph() {
                            for px in 0..s.pw() {
                                let b = (r + py0 + py) * bw + ev.bx + px0 + px;
                                let q8: [[i32; 64]; NCH] = core::array::from_fn(|ch| {
                                    sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap()
                                });
                                let nz8: [u64; NCH] =
                                    core::array::from_fn(|ch| sc.nz[b * NCH + ch]);
                                let aq8 = sc.aq.get(b).copied();
                                try_push(
                                    &mut cg_backups[ci],
                                    MergedBackup {
                                        b,
                                        q8,
                                        nz8,
                                        aq8,
                                        ok: ev.ok[py0 + py][px0 + px],
                                        solo: ev.solo[py0 + py][px0 + px],
                                    },
                                )?;
                            }
                        }
                        let ci_c = ev.cand_ix[s as usize][py0 * 2 + px0].expect("candidate");
                        write_merged(&mut sc, s, r + py0, ev.bx + px0, &ev.cands[ci_c]);
                        for py in 0..s.ph() {
                            for px in 0..s.pw() {
                                covered[(py0 + py) * 2 + px0 + px] = true;
                            }
                        }
                    }
                    if opts.small.is_some() {
                        for py in 0..rr {
                            for px in 0..ev.cols {
                                if covered[py * 2 + px] || !ev.ok[py][px] {
                                    continue;
                                }
                                let so = &ev.solo[py][px];
                                let b = (r + py) * bw + ev.bx + px;
                                for ch in 0..NCH {
                                    if so.split >> ch & 1 != 0 {
                                        try_push(
                                            &mut written,
                                            Written {
                                                b,
                                                ch,
                                                q8: sc.q[(b * NCH + ch) * 64..][..64]
                                                    .try_into()
                                                    .unwrap(),
                                                nz8: sc.nz[b * NCH + ch],
                                                gain: so.gain[ch],
                                            },
                                        )?;
                                    }
                                }
                                write_split(&mut sc, b, so);
                                if self.learned_flags.is_none() {
                                    flags.add(so.split, ac_of(&sc, b));
                                }
                            }
                        }
                    }
                    prev_sym_cg[ci][gc] = sym_out;
                }
            }
            has_prev_row = true;
        }
        // Unit-level merge pruning: revert column groups whose merges do not
        // repay the exact V4 range-coded partition map overhead.
        if opts.merges {
            for ci in 0..n_cg {
                if cg_backups[ci].is_empty() {
                    continue;
                }
                let gs = ci * gw;
                let ge = (gs + gw).min(bw);
                let syms = &cg_syms[ci];
                let map_bits = super::map::encode_v4_partitions(nby, ge - gs, |i| syms[i])
                    .map_or(f64::INFINITY, |m| (m.len() as f64 + 2.0) * 8.0);
                if cg_merge_gain[ci] < map_bits {
                    for bk in &cg_backups[ci] {
                        sc.modes[bk.b] = 0;
                        if let Some(a) = bk.aq8 {
                            sc.aq[bk.b] = a;
                        }
                        for ch in 0..NCH {
                            sc.q[(bk.b * NCH + ch) * 64..][..64].copy_from_slice(&bk.q8[ch]);
                            sc.nz[bk.b * NCH + ch] = bk.nz8[ch];
                        }
                        if opts.small.is_some() && bk.ok {
                            for ch in 0..NCH {
                                if bk.solo.split >> ch & 1 != 0 {
                                    try_push(
                                        &mut written,
                                        Written {
                                            b: bk.b,
                                            ch,
                                            q8: bk.q8[ch],
                                            nz8: bk.nz8[ch],
                                            gain: bk.solo.gain[ch],
                                        },
                                    )?;
                                }
                            }
                            write_split(&mut sc, bk.b, &bk.solo);
                        }
                    }
                }
            }
        }
        if opts.small.is_some_and(|f| f.is_none()) {
            prune_units(&mut sc, group_bw, &written);
        }
        stop.check().map_err(|_| crate::Error::Cancelled)?;
        Ok(sc)
    }
}

/// Trellis-quantize a single 64-slot candidate AC unit under `coder`.
#[allow(clippy::too_many_arguments)]
fn trellis_unit(
    coder: &Coder,
    src: &[f32; 64],
    steps_aq: &[f32; 64],
    ch: usize,
    r: Refs,
    lam: &LossyTrellis,
    norm_override: Option<f64>,
    lambda_mul: f64,
    cands: &mut crate::lossy::trellis::Cands,
    q: &mut [i32; 64],
    nz: &mut u64,
) {
    match coder {
        Coder::RunLevel(p) => {
            crate::lossy::trellis::trellis_unit_rl(
                src,
                steps_aq,
                ch,
                r.eob,
                p,
                lam,
                norm_override,
                lambda_mul,
                cands,
                q,
                nz,
            );
        }
        Coder::SplitHuf(h) => {
            crate::lossy::trellis::trellis_unit_sh(
                src,
                steps_aq,
                ch,
                r.count,
                h,
                lam,
                norm_override,
                lambda_mul,
                q,
                nz,
            );
        }
    }
}

/// Surrounding-block adaptive RDO version of [`solo_parent`]: supports
/// trellis-priced solo 8x8/4x4 candidates under unified group lambda.
#[allow(clippy::too_many_arguments)]
fn solo_parent_rdo(
    sc: &StripeCoefs,
    enc: &Enc,
    planes: &[Vec<f32>; 2],
    coefs: &[Vec<f32>; 2],
    pw: usize,
    g: &Group2,
    (py, px): (usize, usize),
    w: &[f64; NCH],
    group_norm: &[f64; NCH],
    lambda_mul: f64,
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    lam: &LossyTrellis,
    small: Option<(Option<u8>, u8)>,
    opts: &Opts,
    flags: &FlagCost,
    prev_refs: &dyn Fn(&StripeCoefs, usize) -> Refs,
    tcands: &mut crate::lossy::trellis::Cands,
) -> Solo {
    let tr_on = opts.rdo.trellis_select && opts.trellis;
    let bw = sc.bw;
    let b = (g.r + py) * bw + g.bx + px;
    let aq = sc.aq.get(b).copied().unwrap_or(64) as f32 / 64.0;
    let r0 = prev_refs(sc, b);

    let mut q8: [[i32; 64]; NCH] =
        core::array::from_fn(|ch| sc.q[(b * NCH + ch) * 64..][..64].try_into().unwrap());
    let mut m8 = q8.map(|q| nonzero(&q));
    if tr_on && sc.aq.get(b) != Some(&0) {
        let mut lref = r0;
        for ch in 0..NCH {
            let c: &[f32; 64] = coefs[py][(ch * bw + g.bx + px) * 64..][..64]
                .try_into()
                .unwrap();
            let steps_aq: [f32; 64] = core::array::from_fn(|i| {
                if i == 0 {
                    enc.steps8[ch][0]
                } else {
                    enc.steps8[ch][i] * aq
                }
            });
            let mut nz = m8[ch] | u64::from(q8[ch][0] != 0);
            let rf = if ch == 0 { r0 } else { lref };
            trellis_unit(
                coder,
                c,
                &steps_aq,
                ch,
                rf,
                lam,
                Some(group_norm[ch]),
                lambda_mul,
                tcands,
                &mut q8[ch],
                &mut nz,
            );
            m8[ch] = nz & !1;
            if ch == 0 {
                lref = Refs::of_mask(m8[0]);
            }
        }
    }

    let (mut d8, mut x8) = ([0.0f64; NCH], [0.0f64; NCH]);
    let mut q4 = [[0i32; 64]; NCH];
    let (mut m4, mut d4, mut x4) = ([0u64; NCH], [0.0f64; NCH], [0.0f64; NCH]);
    let kx = (opts.kappa - 1.0) as f64;
    let mut lref4 = Refs::of_mask(m8[0]);

    for ch in 0..NCH {
        let c: &[f32; 64] = coefs[py][(ch * bw + g.bx + px) * 64..][..64]
            .try_into()
            .unwrap();
        let st = &enc.steps8[ch];
        let first = st[1] * aq;
        let (d, en) = incant!(
            terms_impl(c, &q8[ch], (&enc.inv8[ch], st), aq),
            [v3, neon, wasm128, scalar]
        );
        let ring8 = RING * ring_unit(first, wt[ch][py][px], en);
        (d8[ch], x8[ch]) = (w[ch] * (d + ring8), w[ch] * kx * (d + ring8));

        if small.is_some_and(|(_, cand)| cand >> ch & 1 != 0) {
            let mut pix = [0.0f32; 64];
            for (y, row) in pix.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                row.copy_from_slice(&planes[py][ch * 8 * pw + y * pw + (g.bx + px) * 8..][..8]);
            }
            let cf = (opts.forward)(&pix);
            let (mut d_c4, mut en_c4) = incant!(
                cand4_impl(
                    &cf,
                    (&enc.inv4[ch], &enc.steps4[ch]),
                    (aq, round),
                    &mut q4[ch]
                ),
                [v3, neon, wasm128, scalar]
            );
            q4[ch][0] = q8[ch][0];
            m4[ch] = nonzero(&q4[ch]);
            if tr_on && sc.aq.get(b) != Some(&0) {
                let steps4_aq: [f32; 64] = core::array::from_fn(|i| {
                    if i == 0 {
                        enc.steps4[ch][0]
                    } else {
                        enc.steps4[ch][i] * aq
                    }
                });
                let mut nz = m4[ch] | u64::from(q4[ch][0] != 0);
                let rf = if ch == 0 { r0 } else { lref4 };
                trellis_unit(
                    coder,
                    &cf,
                    &steps4_aq,
                    ch,
                    rf,
                    lam,
                    Some(group_norm[ch]),
                    lambda_mul,
                    tcands,
                    &mut q4[ch],
                    &mut nz,
                );
                m4[ch] = nz & !1;
                if ch == 0 && m4[0] != 0 {
                    lref4 = Refs::of_mask(m4[0]);
                }
                let (dt, et) = incant!(
                    terms_impl(&cf, &q4[ch], (&enc.inv4[ch], &enc.steps4[ch]), aq),
                    [v3, neon, wasm128, scalar]
                );
                d_c4 = dt;
                en_c4 = et;
            }
            let ring4 = RING * ring_unit(first, wt[ch][py][px], en_c4);
            (d4[ch], x4[ch]) = (w[ch] * (d_c4 + ring4), w[ch] * kx * (d_c4 + ring4));
            if opts.edge > 0.0 {
                let first64 = first as f64;
                let k = w[ch] * (opts.kappa * opts.edge) as f64 / (first64 * first64);
                x8[ch] += k * inner_edge(&pix, &recon(&q8[ch], &enc.dq8[ch], aq, false));
                x4[ch] += k * inner_edge(&pix, &recon(&q4[ch], &enc.dq4[ch], aq, true));
            }
        }
    }

    let unit = |ch: usize, s: usize| -> (&[i32; 64], u64) {
        if s == 0 {
            (&q8[ch], m8[ch])
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
        let mut j = 0.0;
        for ch in 0..NCH {
            j += d8[ch] + if ch == 0 { lbits(0) } else { cbits(ch, 0, 0) };
        }
        best = (j, 0, 0.0);
    }
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

/// Surrounding-block adaptive RDO version of [`merged`]: supports
/// trellis-priced merged superblock units under unified group lambda.
#[allow(clippy::too_many_arguments)]
fn merged_rdo(
    sc: &StripeCoefs,
    enc: &Enc,
    sp: &[Spectra; NCH],
    g: &Group2,
    (s, py0, px0): (Shape, usize, usize),
    w: &[f64; NCH],
    group_norm: &[f64; NCH],
    lambda_mul: f64,
    wt: &[[[f32; 2]; 2]; NCH],
    round: f32,
    coder: &Coder,
    lam: &LossyTrellis,
    opts: &Opts,
    prev_refs: &dyn Fn(usize) -> Refs,
    tcands: &mut crate::lossy::trellis::Cands,
    out: &mut MCand,
    spec_err: &mut [[f32; 256]; NCH],
) {
    let tr_on = opts.rdo.trellis_select && opts.trellis;
    let bw = sc.bw;
    let b_of = |py: usize, px: usize| (g.r + py0 + py) * bw + g.bx + px0 + px;
    let aqc = (0..s.ph())
        .flat_map(|py| (0..s.pw()).map(move |px| (py, px)))
        .map(|(py, px)| sc.aq.get(b_of(py, px)).copied().unwrap_or(64))
        .min()
        .unwrap();
    let aq = aqc as f32 / 64.0;
    out.shape = s;
    out.j = 0.0;
    out.aq = aqc;
    let si = s as usize;
    let gather = &enc.gather[si];
    let n_p = s.ph() * s.pw();

    if tr_on {
        let mut lrefs = [Refs { eob: 0, count: 0 }; 4];
        for ch in 0..NCH {
            let c = sp[ch].get(s, py0, px0);
            let dd = llf_to_dcs(s, [[c[0], c[1]], [c[16], c[17]]]);
            for py in 0..s.ph() {
                for px in 0..s.pw() {
                    let v = dd[py][px] / enc.steps8[ch][0];
                    let r = crate::mathx::round_pos(v.abs() as f64).min(4_194_303.0) as i32;
                    out.dcs[ch][py][px] = if v < 0.0 { -r } else { r };
                }
            }
            let st = &enc.steps[ch][si];
            let inv = &enc.inv[ch][si];
            let sc_k = &enc.sc[si];
            let mut qs = [0i32; 256];
            for p in 0..n_p {
                let mut usrc = [0.0f32; 64];
                let mut usteps = [1.0f32; 64];
                for z in 1..64 {
                    let idx = gather[p * 64 + z] as usize;
                    usrc[z] = c[idx];
                    usteps[z] = st[idx] * aq;
                }
                let b = b_of(p / s.pw(), p % s.pw());
                let rf = if ch == 0 { prev_refs(b) } else { lrefs[p] };
                out.units[ch][p] = [0i32; 64];
                let mut nz = 0u64;
                trellis_unit(
                    coder,
                    &usrc,
                    &usteps,
                    ch,
                    rf,
                    lam,
                    Some(group_norm[ch]),
                    lambda_mul,
                    tcands,
                    &mut out.units[ch][p],
                    &mut nz,
                );
                if ch == 0 {
                    lrefs[p] = Refs::of_mask(nz & !1);
                }
                for z in 1..64 {
                    qs[gather[p * 64 + z] as usize] = out.units[ch][p][z];
                }
            }
            spec_err[ch].fill(0.0);
            let inv_aq = 1.0 / aq;
            let mut d = 0.0f64;
            for v in 0..s.h() {
                for u in 0..s.w() {
                    if v < s.ph() && u < s.pw() {
                        continue;
                    }
                    let idx = v * 16 + u;
                    let cv = c[idx];
                    let qv = qs[idx] as f32;
                    let e_step = cv * inv[idx] * inv_aq - qv;
                    d += (e_step as f64) * (e_step as f64);
                    spec_err[ch][idx] = (cv - qv * st[idx] * aq) * sc_k[idx];
                }
            }
            out.j += w[ch] * d;
        }
    } else {
        let mut qs = [0i32; 256];
        for ch in 0..NCH {
            let c = sp[ch].get(s, py0, px0);
            let tabs = (&enc.inv[ch][si], &enc.steps[ch][si], &enc.sc[si]);
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
            for (p, unit) in out.units[ch].iter_mut().enumerate().take(n_p) {
                for (z, v) in unit.iter_mut().enumerate() {
                    *v = qs[gather[p * 64 + z] as usize];
                }
            }
            out.j += w[ch] * d as f64;
            let dd = llf_to_dcs(s, [[c[0], c[1]], [c[16], c[17]]]);
            for py in 0..s.ph() {
                for px in 0..s.pw() {
                    let v = dd[py][px] / enc.steps8[ch][0];
                    let r = crate::mathx::round_pos(v.abs() as f64).min(4_194_303.0) as i32;
                    out.dcs[ch][py][px] = if v < 0.0 { -r } else { r };
                }
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
