//! Lossy stripe entropy decoding, coefficient dequantization/IDCT,
//! shared-DC scheduling, color reconstruction, and frame decoding.

use super::parse::*;
use crate::lossy::*;

/// Quantized DCs from DC-image sample bytes (U16 or I32).
/// Quantized DCs (and, for a 4-channel DC image, the per-block AQ step
/// multipliers `a / 64`, a clamped to 0..=255) from DC-image sample bytes.
/// 16-bit DC samples (value + 32768) -> quantized DC (no clamp needed).
#[archmage::autoversion]
pub(in crate::lossy) fn dc_u16(px: &[u8], out: &mut [i32]) {
    for (o, c) in out.iter_mut().zip(px.as_chunks::<2>().0) {
        *o = u16::from_ne_bytes(*c) as i32 - 32768;
    }
}

pub(in crate::lossy) fn resize_fallible<T: Clone>(
    v: &mut Vec<T>,
    n: usize,
    value: T,
) -> Result<(), Error> {
    v.try_reserve(n.saturating_sub(v.len()))
        .map_err(|_| Error::TooLarge)?;
    v.resize(n, value);
    Ok(())
}

/// DC values of nested DC rows and, with a fourth (block code) channel,
/// per-block scales: AQ multipliers code / 64 (0 also keeps the reference
/// block), or with `exact` 1.0 for code 0 and 0.0 (not reconstructed) for an
/// exact block (code 1); any other exact code is Corrupt.
pub(in crate::lossy) fn dc_values(
    sample: SampleType,
    aq: bool,
    exact: bool,
    px: &[u8],
    out: &mut Vec<i32>,
    scales: &mut Vec<f32>,
) -> Result<(), Error> {
    if sample == SampleType::U16 && !aq {
        let start = out.len();
        resize_fallible(
            out,
            start.checked_add(px.len() / 2).ok_or(Error::TooLarge)?,
            0,
        )?;
        dc_u16(px, &mut out[start..]);
        return Ok(());
    }
    let lim = 1 << 22;
    let mut raw = Vec::new();
    raw.try_reserve_exact(px.len() / sample.bytes())
        .map_err(|_| Error::TooLarge)?;
    match sample {
        SampleType::U16 => raw.extend(
            px.as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_ne_bytes(c) as i32),
        ),
        _ => raw.extend(px.as_chunks::<4>().0.iter().map(|&c| i32::from_ne_bytes(c))),
    }
    let off = if sample == SampleType::U16 { 32768 } else { 0 };
    let n = if aq { 4 } else { 3 };
    let blocks = raw.len() / n;
    out.try_reserve(blocks.checked_mul(3).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    if aq {
        scales.try_reserve(blocks).map_err(|_| Error::TooLarge)?;
    }
    for b in raw.chunks_exact(n) {
        out.extend(b[..3].iter().map(|&v| (v - off).clamp(-lim, lim)));
        if exact {
            scales.push(match b[3] {
                0 => 1.0,
                1 => 0.0,
                _ => return Err(Error::Corrupt("exact block code")),
            });
        } else if aq {
            scales.push(b[3].clamp(0, 255) as f32 * (1.0 / 64.0));
        }
    }
    Ok(())
}

/// Split-Huffman: magnitudes and signs of every coded group, blocks in
/// order.
/// Each group's magnitudes come from one 64-bit window (len * w <= 57; wider
/// groups take a per-coefficient path) and its signs from one window of the
/// sign stream, the i-th nonzero taking bit i.
#[inline(never)]
pub(in crate::lossy) fn split_unpack(
    mag: &[u8],
    sgn: &[u8],
    nblocks: usize,
    counts: &[u8],
    wid: &[u8],
    q: &mut [i32],
    simd: bool,
) -> Result<(), Error> {
    let nbc = nblocks * NCH;
    let (mut om, mut os) = (0u64, 0u64);
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    let done = if simd
        && !crate::v1::force_scalar()
        && !crate::v1::force_avx2()
        && let Some(t) = <archmage::X64V4xToken as archmage::SimdToken>::summon()
    {
        (om, os) = split_avx512::unpack(t, mag, sgn, nbc, counts, wid, q);
        true
    } else {
        false
    };
    #[cfg(not(all(target_arch = "x86_64", feature = "avx512")))]
    let done = {
        let _ = simd;
        false
    };
    #[cfg(target_arch = "x86_64")]
    let done = done
        || (simd
            && !crate::v1::force_scalar()
            && match <archmage::X64V3Token as archmage::SimdToken>::summon() {
                Some(t) => {
                    (om, os) = split_avx2::unpack(t, mag, sgn, nbc, counts, wid, q);
                    true
                }
                None => false,
            });
    if !done {
        for (bc, qb) in q[..nbc * 64].as_chunks_mut::<64>().0.iter_mut().enumerate() {
            let n = (counts[bc] as usize).min(coef::PK_GROUPS);
            let ws: &[u8; coef::PK_GROUPS] = wid[bc * coef::PK_GROUPS..][..coef::PK_GROUPS]
                .try_into()
                .unwrap();
            (om, os) = split_unpack_bc(mag, sgn, n, ws, qb, om, os);
        }
    }
    if om > mag.len() as u64 * 8 || os > sgn.len() as u64 * 8 {
        return Err(Error::Corrupt("split bit stream overrun"));
    }
    Ok(())
}

/// One block-channel's `n` groups from bit offsets (`om`, `os`); returns
/// the offsets after it.
#[inline(always)]
pub(crate) fn split_unpack_bc(
    mag: &[u8],
    sgn: &[u8],
    n: usize,
    ws: &[u8; coef::PK_GROUPS],
    qb: &mut [i32; 64],
    mut om: u64,
    mut os: u64,
) -> (u64, u64) {
    for gi in 0..n {
        (om, os) = split_group(mag, sgn, gi, ws[gi] as u32, qb, om, os);
    }
    (om, os)
}

/// Group `gi` (width `w`) of a block-channel from bit offsets (`om`, `os`);
/// returns the offsets after it.
#[inline(always)]
pub(crate) fn split_group(
    mag: &[u8],
    sgn: &[u8],
    gi: usize,
    w: u32,
    qb: &mut [i32; 64],
    mut om: u64,
    mut os: u64,
) -> (u64, u64) {
    // Group 15 has three coefficients.
    let len = 4 - (gi == coef::PK_GROUPS - 1) as u32;
    let field = len * w;
    let mask = (1u64 << w) - 1;
    let m: [u32; 4] = if field <= 57 {
        let f = rawbits::get(mag, om, field);
        core::array::from_fn(|j| ((f >> (j as u32 * w)) & mask) as u32)
    } else {
        core::array::from_fn(|j| {
            if (j as u32) < len {
                rawbits::get(mag, om + (j as u32 * w) as u64, w) as u32
            } else {
                0
            }
        })
    };
    om += field as u64;
    let nzm = m.map(|v| (v != 0) as u32);
    let nz = nzm[0] + nzm[1] + nzm[2] + nzm[3];
    let s = rawbits::get(sgn, os, nz) as u32;
    os += nz as u64;
    let mut rank = 0;
    for j in 0..4 {
        let neg = 0u32.wrapping_sub((s >> rank) & nzm[j]);
        rank += nzm[j];
        let v = ((m[j] ^ neg).wrapping_sub(neg)) as i32;
        if (j as u32) < len {
            qb[ZIGZAG[1 + 4 * gi + j] as usize & 63] = v;
        }
    }
    (om, os)
}

#[derive(Clone, Copy, Default)]
pub(in crate::lossy) struct Cursor {
    /// Block-row offset within the stripe and block column.
    pub(in crate::lossy) r: u32,
    pub(in crate::lossy) bx: u32,
    pub(in crate::lossy) ch: u8,
    /// Next AC index (0 = expecting the EOB token).
    pub(in crate::lossy) k: u8,
    pub(in crate::lossy) eob: u8,
    /// Luma EOB of the current block / previous luma EOB of the lane.
    pub(in crate::lossy) luma: u8,
    pub(in crate::lossy) done: bool,
}

/// Scalar reference: entropy-decodes stripe `s` into quantized
/// coefficients `[(block * 3 + ch) * 64 + raster]` (DC from `dc`) plus the
/// EOB of each (block, ch). `q` has 64 and `eobs` 1 spare slots at the end
/// (write targets for inactive SIMD lanes).
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn decode_stripe_coefs(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    u: usize,
    dc: &[i32],
    simd: bool,
    sc: &mut Scratch,
) -> Result<(), Error> {
    let q = core::mem::take(&mut sc.q);
    let eobs = core::mem::take(&mut sc.eobs);
    let r = decode_stripe_raw_bytes(p, g, s, p.stripes[u], Group::ALL, simd, q, eobs)?;
    finish_coefs(p, g, s, dc, r, &mut sc.q, &mut sc.eobs)
}

/// After the entropy decode of stripe `s`: clears the spare slots, checks
/// for a stream overrun and fills in the DCs.
pub(in crate::lossy) fn finish_coefs(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    dc: &[i32],
    (mut q, eobs, wp, len): (Vec<i32>, Vec<u8>, usize, usize),
    sq: &mut Vec<i32>,
    seobs: &mut Vec<u8>,
) -> Result<(), Error> {
    let n = q.len();
    // The spare slots are never read; clear them for the next stripe.
    q[n - 64..].fill(0);
    *sq = q;
    *seobs = eobs;
    if wp > len {
        sq.fill(0);
        return Err(Error::Corrupt("lane stream overrun"));
    }
    let _ = p;
    // Empty `dc`: shared DC rows the caller fills in after their wait.
    if !dc.is_empty() {
        let (by0, by1) = g.rows(s);
        fill_dc(sq, dc, by1 - by0, g.bw);
    }
    Ok(())
}

/// Entropy-decodes stripes `s[0]` and `s[1]` into `sc.q`/`sc.eobs` and
/// `sc.q2`/`sc.eobs2`: through the two-stripe AVX-512 kernel (two
/// independent dependency chains per loop), else one stripe after the
/// other. Same coefficients either way.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn decode_stripe_coefs2(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: [usize; 2],
    u: [usize; 2],
    dc: [&[i32]; 2],
    simd: bool,
    sc: &mut Scratch,
) -> Result<(), Error> {
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if simd
        && p.layouts == [LossyChannelLayout::default(); 3]
        && p.tables.mode == LossyEntropy::RunLevel
        && !crate::v1::force_scalar()
        && !crate::v1::force_avx2()
        && let Some(t) = <archmage::X64V4Token as archmage::SimdToken>::summon()
        && kernel_choice::choice().rl != kernel_choice::Rl::Avx2
    {
        let hw = kernel_choice::choice().rl == kernel_choice::Rl::Avx512Hw;
        let bw = g.bw;
        let active = g.active_lanes();
        let nby = s.map(|s| {
            let (a, b) = g.rows(s);
            b - a
        });
        // The records' bytes after their partition maps (superblocks,
        // small transforms).
        let body = |u: usize| {
            p.record(g, u / p.ngroups(), p.stripes[u])
                .map(|(_, body)| body)
        };
        let unit0 = local_ans::unit(p, body(u[0])?)?;
        let unit1 = local_ans::unit(p, body(u[1])?)?;
        let model0 = unit0
            .model()
            .and_then(|m| m.rans.as_ref())
            .or(p.rans.as_ref())
            .expect("ANS model");
        let model1 = unit1
            .model()
            .and_then(|m| m.rans.as_ref())
            .or(p.rans.as_ref())
            .expect("ANS model");
        fn maps(model: &RansTables) -> entropy_avx512::Maps<'_> {
            entropy_avx512::Maps {
                cmap: &model.cmap,
                cmap32: &model.cmap32,
                dec: &model.dec,
            }
        }
        let m0 = if unit0.model().is_none() {
            entropy_avx512::Maps {
                cmap: p.cmap(),
                cmap32: p.cmap32(),
                dec: p.dec(),
            }
        } else {
            maps(model0)
        };
        let m1 = if unit1.model().is_none() {
            entropy_avx512::Maps {
                cmap: p.cmap(),
                cmap32: p.cmap32(),
                dec: p.dec(),
            }
        } else {
            maps(model1)
        };
        let w0 = unit0.words(active)?;
        let w1 = unit1.words(active)?;
        let mut q = [core::mem::take(&mut sc.q), core::mem::take(&mut sc.q2)];
        let mut e = [
            core::mem::take(&mut sc.eobs),
            core::mem::take(&mut sc.eobs2),
        ];
        for k in 0..2 {
            // Arrive all zero (any length); size to the stripe plus spares.
            q[k].resize(nby[k] * bw * NCH * 64 + 64, 0);
            e[k].clear();
            e[k].resize(nby[k] * bw * NCH + 1, 0);
        }
        let [q0, q1] = &mut q;
        let [e0, e1] = &mut e;
        let bk = [
            eob_bucket_proxy(nby[0] * bw, bw),
            eob_bucket_proxy(nby[1] * bw, bw),
        ];
        let bkt = match &bk {
            [Some(a), Some(b)] => Some([&a[..], &b[..]]),
            _ => None,
        };
        let wp = if core::ptr::eq(model0, model1) {
            entropy_avx512::decode_stripes2(
                t,
                hw,
                [&w0.0, &w1.0],
                [w0.1, w1.1],
                &m0,
                bw,
                nby,
                active,
                0,
                NCH as u32,
                [&mut q0[..], &mut q1[..]],
                [&mut e0[..], &mut e1[..]],
                bkt,
            )
        } else {
            entropy_avx512::decode_stripes2_models(
                t,
                hw,
                [&w0.0, &w1.0],
                [w0.1, w1.1],
                [&m0, &m1],
                bw,
                nby,
                active,
                0,
                NCH as u32,
                [&mut q0[..], &mut q1[..]],
                [&mut e0[..], &mut e1[..]],
                bkt,
            )
        };
        let [q0, q1] = q;
        let [e0, e1] = e;
        let r0 = finish_coefs(
            p,
            g,
            s[0],
            dc[0],
            (q0, e0, wp[0], w0.0.len),
            &mut sc.q,
            &mut sc.eobs,
        );
        let r1 = finish_coefs(
            p,
            g,
            s[1],
            dc[1],
            (q1, e1, wp[1], w1.0.len),
            &mut sc.q2,
            &mut sc.eobs2,
        );
        return r0.and(r1);
    }
    decode_stripe_coefs(p, g, s[0], u[0], dc[0], simd, sc)?;
    core::mem::swap(&mut sc.q, &mut sc.q2);
    core::mem::swap(&mut sc.eobs, &mut sc.eobs2);
    let r = decode_stripe_coefs(p, g, s[1], u[1], dc[1], simd, sc);
    core::mem::swap(&mut sc.q, &mut sc.q2);
    core::mem::swap(&mut sc.eobs, &mut sc.eobs2);
    r
}

/// Per-thread stripe buffers. Invariant between stripes: `q` is all zero
/// (reconstruction zeroes each block after reading it).
#[derive(Default)]
pub(crate) struct Scratch {
    pub(in crate::lossy) q: Vec<i32>,
    pub(in crate::lossy) eobs: Vec<u8>,
    /// Second stripe of a pair (two-stripe entropy decode).
    pub(in crate::lossy) q2: Vec<i32>,
    pub(in crate::lossy) eobs2: Vec<u8>,
    pub(in crate::lossy) dc2: Vec<i32>,
    pub(in crate::lossy) aq2: Vec<f32>,
    /// Color planes of one block row (fully overwritten per row).
    pub(in crate::lossy) planes: Vec<f32>,
    /// One row of linear output (float / 16-bit XYB paths).
    pub(in crate::lossy) lin: Vec<f32>,
    /// Nested DC / alpha stripe pixels and the stripe's DC values.
    pub(in crate::lossy) dc_px: Vec<u8>,
    pub(in crate::lossy) dc: Vec<i32>,
    pub(in crate::lossy) aq: Vec<f32>,
    pub(in crate::lossy) alpha: Vec<u8>,
    /// RGB rows of a stripe for gray / alpha layouts.
    pub(in crate::lossy) tmp: Vec<u8>,
    /// Superblocks: a unit's merge roles.
    pub(in crate::lossy) roles: Vec<u8>,
    /// Small transforms: a unit's 4x4 plane bits per block.
    pub(in crate::lossy) splits: Vec<u8>,
}

/// Entropy decode of stripe `s` without the DC or the overrun check:
/// (coefficients, EOBs, words consumed, words available).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
#[allow(clippy::type_complexity)]
pub(in crate::lossy) fn decode_stripe_raw(
    p: &LossyParsed<'_>,
    s: usize,
    grp: Group,
    simd: bool,
    q: Vec<i32>,
    eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    decode_stripe_raw_bytes(p, &p.g, s, p.stripes[s], grp, simd, q, eobs)
}

#[allow(clippy::type_complexity)]
pub(in crate::lossy) fn decode_stripe_raw_bytes(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    grp: Group,
    simd: bool,
    q: Vec<i32>,
    eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    let (modes, bytes) = p.record(g, s, bytes)?;
    let result = decode_stripe_raw_inner(p, g, s, bytes, grp, simd, q, eobs)?;
    // Sampled transforms use a strict compact prefix. Reject hidden values in
    // discarded slots instead of permitting multiple unchecked representations.
    if p.layouts
        .iter()
        .any(|l| l.sampling() != ChannelSampling::Full)
    {
        let (a, b) = g.rows(s);
        for block in 0..(b - a) * g.bw {
            for ch in 0..grp.nch {
                let channel = grp.ch0 + ch;
                let l = p.layouts[channel].layout(modes.get(block) & (1 << channel) != 0);
                for &i in &ZIGZAG[l.samples()..] {
                    if result.0[(block * grp.nch + ch) * 64 + i as usize] != 0 {
                        return Err(Error::Corrupt("nonzero unused sampled coefficient"));
                    }
                }
            }
        }
    }
    Ok(result)
}

#[allow(clippy::type_complexity)]
pub(in crate::lossy) fn decode_stripe_raw_inner(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    grp: Group,
    simd: bool,
    q: Vec<i32>,
    eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    let unit = local_ans::unit(p, bytes)?;
    decode_coefficient_unit(
        g,
        s,
        &unit,
        AcModelView {
            tables: &p.tables,
            rans: p.rans.as_ref(),
            huf: p.huf.as_ref(),
        },
        grp,
        simd,
        q,
        eobs,
    )
}

/// Borrowed probability tables, independent of the surrounding wire grammar.
pub(in crate::lossy) struct AcModelView<'a> {
    pub(in crate::lossy) tables: &'a Tables,
    pub(in crate::lossy) rans: Option<&'a RansTables>,
    pub(in crate::lossy) huf: Option<&'a (huf::HufTables, huf::Dec, huf::WidRows)>,
}

#[allow(clippy::type_complexity)]
pub(in crate::lossy) fn decode_coefficient_unit(
    g: &Geom,
    s: usize,
    unit: &local_ans::Unit<'_>,
    root: AcModelView<'_>,
    grp: Group,
    simd: bool,
    mut q: Vec<i32>,
    mut eobs: Vec<u8>,
) -> Result<(Vec<i32>, Vec<u8>, usize, usize), Error> {
    let (by0, by1) = g.rows(s);
    let nby = by1 - by0;
    let bw = g.bw;
    // `q` arrives all zero (any length); size it to this stripe plus the
    // 64 spare slots.
    let nch = grp.nch;
    let (ch0, nchu) = (grp.ch0 as u32, nch as u32);
    q.resize(nby * bw * nch * 64 + 64, 0);
    eobs.clear();
    eobs.resize(nby * bw * nch + 1, 0);
    let active = g.active_lanes();
    let model = unit.model();
    let chosen_huf = model.map_or(root.huf, |m| m.huf.as_ref());
    let chosen_tables = model.map_or(root.tables, |m| &m.tables);
    let chosen_rans = model.map_or(root.rans, |m| m.rans.as_ref());
    let bytes = unit.body;
    if let Some((ht, d, rows)) = chosen_huf {
        let _ = (ch0, nchu, active);
        let t = prof::start();
        let mut r = ByteReader::new(bytes);
        let mut sec = [&[][..]; 3];
        for v in sec.iter_mut() {
            let n = r.var_usize()?;
            *v = r.bytes(n)?;
        }
        let n = r.var_usize()?;
        let mag = r.bytes(n)?;
        let sgn = r.bytes(r.remaining())?;
        let nb = nby * bw;
        let mut wid = Vec::new();
        resize_fallible(&mut wid, nb * NCH * coef::PK_GROUPS + coef::PK_GROUPS, 0)?;
        let mut hs = huf::Scratch::default();
        huf::decode_side(ht, d, rows, sec, nb, &mut eobs, &mut wid, &mut hs)?;
        t.add(4);
        let t = prof::start();
        split_unpack(mag, sgn, nb, &eobs, &wid, &mut q, simd)?;
        t.add(5);
        return Ok((q, eobs, 0, 0));
    }
    let tables = chosen_tables;
    let rans = chosen_rans.expect("ANS tables");
    let (words, mut x) = unit.words(active)?;
    #[cfg(target_arch = "x86_64")]
    if simd && !crate::v1::force_scalar() {
        use archmage::SimdToken;
        if !crate::v1::force_avx2()
            && let Some(t) = archmage::X64V4Token::summon()
            && kernel_choice::choice().rl != kernel_choice::Rl::Avx2
        {
            let m = entropy_avx512::Maps {
                cmap: &rans.cmap,
                cmap32: &rans.cmap32,
                dec: &rans.dec,
            };
            let hw = kernel_choice::choice().rl == kernel_choice::Rl::Avx512Hw;
            let bk = eob_bucket_proxy(nby * bw, bw);
            let wp = entropy_avx512::decode_stripe(
                t,
                hw,
                &words,
                x,
                &m,
                bw,
                nby,
                active,
                ch0,
                nchu,
                &mut q,
                &mut eobs,
                bk.as_deref(),
            );
            return Ok((q, eobs, wp, words.len));
        }
        if let Some(t) = archmage::X64V3Token::summon() {
            let wp = entropy_avx2::decode_stripe(
                t, &words, x, &rans.cmap, &rans.dec, bw, nby, active, ch0, nchu, &mut q, &mut eobs,
            );
            return Ok((q, eobs, wp, words.len));
        }
    }
    #[cfg(target_arch = "aarch64")]
    if simd && !crate::v1::force_scalar() {
        use archmage::SimdToken;
        if let Some(t) = archmage::NeonToken::summon() {
            let bk = eob_bucket_proxy(nby * bw, bw);
            let wp = entropy_neon::decode_stripe(
                t,
                &words,
                x,
                &rans.cmap,
                &rans.dec,
                bw,
                nby,
                active,
                ch0,
                nchu,
                &mut q,
                &mut eobs,
                bk.as_deref(),
            );
            return Ok((q, eobs, wp, words.len));
        }
    }
    #[cfg(target_arch = "wasm32")]
    if simd && !crate::v1::force_scalar() {
        let wp = entropy_portable::decode_stripe(
            &words, x, &rans.cmap, &rans.dec, bw, nby, active, ch0, nchu, &mut q, &mut eobs,
        );
        return Ok((q, eobs, wp, words.len));
    }
    let _ = simd;
    let mut wp = 0usize;
    let mut cur = [Cursor::default(); NL];
    for (l, c) in cur.iter_mut().enumerate() {
        c.bx = l as u32;
        c.done = l >= active || nby == 0;
    }
    let renorm = |x: &mut u32, wp: &mut usize| {
        if *x < RANS_L {
            *x = (*x << 16) | words.word(*wp) as u32;
            *wp += 1;
        }
    };
    let mut sym = [0u32; NL];
    let mut nbits = [0u32; NL];
    let mut live = [false; NL];
    loop {
        let mut any = false;
        // Token phase.
        for l in 0..NL {
            live[l] = !cur[l].done;
            if !live[l] {
                continue;
            }
            any = true;
            let c = cur[l];
            let ch = c.ch as usize;
            let raw = if c.k == 0 {
                eob_ctx(grp.ch0 + ch, c.luma as u32)
            } else {
                ac_ctx(grp.ch0 + ch, c.k as u32, c.eob as u32)
            };
            let t = tables.ctx_map[raw] as usize;
            let xs = x[l];
            let e = rans.dec[(t << LOG_M) | (xs & (M - 1)) as usize];
            let mut nx = ((e & 1023) + 1) * (xs >> LOG_M) + ((e >> 10) & 1023);
            renorm(&mut nx, &mut wp);
            x[l] = nx;
            sym[l] = (e >> 20) & 127;
            nbits[l] = e >> 27;
        }
        if !any {
            break;
        }
        // Extra bits: low part (<= 16), then high part.
        let mut extra = [0u32; NL];
        for l in 0..NL {
            if live[l] && nbits[l] > 0 {
                let n = nbits[l].min(16);
                extra[l] = x[l] & crate::mask_of(n);
                x[l] >>= n;
                renorm(&mut x[l], &mut wp);
            }
        }
        for l in 0..NL {
            if live[l] && nbits[l] > 16 {
                let n = nbits[l] - 16;
                extra[l] |= (x[l] & crate::mask_of(n)) << 16;
                x[l] >>= n;
                renorm(&mut x[l], &mut wp);
            }
        }
        // Apply.
        for l in 0..NL {
            if !live[l] {
                continue;
            }
            let c = &mut cur[l];
            let b = c.r as usize * bw + c.bx as usize;
            let ch = c.ch as usize;
            let mut next_channel = false;
            if c.k == 0 {
                c.eob = sym[l] as u8;
                eobs[b * nch + ch] = c.eob;
                if ch == 0 {
                    c.luma = c.eob;
                }
                if c.eob == 0 {
                    next_channel = true;
                } else {
                    c.k = 1;
                }
            } else if sym[l] == coef::ZRL {
                let k = c.k as u32 + 4;
                if k > c.eob as u32 {
                    next_channel = true;
                } else {
                    c.k = k as u8;
                }
            } else {
                let pos = c.k as u32 + (sym[l] >> 5);
                let e = c.eob as u32;
                if pos <= e {
                    q[(b * nch + ch) * 64 + ZIGZAG[pos as usize] as usize] =
                        rl_value(sym[l] & 31, extra[l]);
                }
                if pos >= e {
                    next_channel = true;
                } else {
                    c.k = (pos + 1) as u8;
                }
            }
            if next_channel {
                c.k = 0;
                c.ch += 1;
                if c.ch as usize == nch {
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
        // Past the end of the stream the decode is already invalid.
        if wp > words.len {
            break;
        }
    }
    Ok((q, eobs, wp, words.len))
}

/// [`aq_dequant`] without the copy when the block has no multiplier or
/// multiplier 1 (identical factors: x * 1.0 == x): `dq` itself, else the
/// scaled factors in `buf`.
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
#[inline(always)]
pub(crate) fn aq_dequant_in<'a>(
    dq: &'a [f32; 64],
    s: Option<f32>,
    buf: &'a mut [f32; 64],
) -> &'a [f32; 64] {
    match s {
        Some(s) if s != 1.0 => {
            *buf = aq_dequant(dq, Some(s));
            buf
        }
        _ => dq,
    }
}

/// Normative AQ dequantization factors: AC factors times the block's
/// multiplier, the DC factor unchanged.
pub(crate) fn aq_dequant(dq: &[f32; 64], s: Option<f32>) -> [f32; 64] {
    let mut d = *dq;
    if let Some(s) = s {
        for v in &mut d[1..] {
            *v *= s;
        }
    }
    d
}

/// Copies the stripe's DCs (`dc`: the stripe's rows of the DC image) into
/// `q`.
pub(in crate::lossy) fn fill_dc(q: &mut [i32], dc: &[i32], nby: usize, bw: usize) {
    for r in 0..nby {
        for bx in 0..bw {
            for ch in 0..NCH {
                q[((r * bw + bx) * NCH + ch) * 64] = dc[(r * bw + bx) * NCH + ch];
            }
        }
    }
}

/// Reconstructs stripe `s` from its record `bytes` into `out` (its first row is the stripe's first
/// pixel row), with the SIMD kernels or the scalar reference. `dc` holds
/// the stripe's rows of the DC image.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn decode_stripe_bytes_mode<const BOUNDED: bool>(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    dc: &[i32],
    aq: &[f32],
    out: &mut [u8],
    stride: usize,
    simd: bool,
    sc: &mut Scratch,
) -> Result<(), Error> {
    let t = prof::start();
    let q = core::mem::take(&mut sc.q);
    let eobs = core::mem::take(&mut sc.eobs);
    let r = decode_stripe_raw_bytes(p, g, s, bytes, Group::ALL, simd, q, eobs)?;
    finish_coefs(p, g, s, dc, r, &mut sc.q, &mut sc.eobs)?;
    t.add(1);
    let Scratch {
        q,
        eobs,
        planes,
        lin,
        roles,
        splits,
        ..
    } = sc;
    let (by0, by1) = g.rows(s);
    let n = (by1 * 8).min(g.h) - by0 * 8;
    let mut rows: Vec<&mut [u8]> = Vec::new();
    rows.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    rows.extend(out.chunks_mut(stride).take(n));
    let ws = (planes, lin, roles, splits);
    recon_unit::<BOUNDED>(p, g, s, bytes, aq, &mut rows, simd, q, eobs, ws)
}

/// Reconstructs stripe `s` (geometry `g`) from its unit record `bytes` and
/// decoded coefficients into `out`: the 8x8 path, with superblocks the
/// merged traversal when the unit's partition map merges anything, with
/// small transforms the 4x4 planes its map splits. A corrupt map or merge
/// clears `q` (the scratch invariant) before it returns the error.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(in crate::lossy) fn recon_unit<const BOUNDED: bool>(
    p: &LossyParsed<'_>,
    g: &Geom,
    s: usize,
    bytes: &[u8],
    aq: &[f32],
    out: &mut [&mut [u8]],
    simd: bool,
    q: &mut [i32],
    eobs: &[u8],
    (planes, lin, roles, splits): (&mut Vec<f32>, &mut Vec<f32>, &mut Vec<u8>, &mut Vec<u8>),
) -> Result<(), Error> {
    let render = p.reconstruction();
    let (a, b) = g.rows(s);
    if let Some(defaults) = &p.coding_defaults {
        let mut scales = Vec::new();
        let (merged, split) =
            defaults.maps(p, bytes, b - a, g.bw, aq, roles, splits, &mut scales)?;
        let small = simd::Small {
            split: if split { splits } else { &[] },
            dq: p.small.as_deref().unwrap_or(&p.dq),
            int: false,
        };
        if merged {
            superblock::recon_stripe::<BOUNDED>(
                &render,
                p.sb.as_deref().expect("declared merges"),
                g,
                s,
                &scales,
                roles,
                out,
                simd,
                q,
                eobs,
                planes,
                lin,
                small,
            );
        } else {
            recon_stripe_mode::<BOUNDED>(
                &render,
                g,
                s,
                &scales,
                layout::ModeMap::Constant(0),
                out,
                simd,
                q,
                eobs,
                planes,
                lin,
                small,
            );
        }
        return Ok(());
    }
    match p.map {
        MapKind::None => {}
        MapKind::Merges => {
            let t = p.sb.as_deref().expect("superblock tables");
            let merged = superblock::read_map(bytes, b - a, g.bw, roles)
                .and_then(|(m, _)| {
                    if m {
                        superblock::validate(roles, aq, g.bw, code0_skips(p))?;
                    }
                    Ok(m)
                })
                .inspect_err(|_| q.fill(0))?;
            if merged {
                superblock::recon_stripe::<BOUNDED>(
                    &render,
                    t,
                    g,
                    s,
                    aq,
                    roles,
                    out,
                    simd,
                    q,
                    eobs,
                    planes,
                    lin,
                    simd::Small::NONE,
                );
                return Ok(());
            }
        }
        MapKind::Joint => {
            let skip = code0_skips(p);
            let (merged, split) = superblock::read_joint_map(
                bytes,
                (b - a, g.bw),
                (aq, skip),
                (q, eobs),
                roles,
                splits,
            )
            .and_then(|(m, sp, _)| {
                if m {
                    // Merges in a joint map are gated syntax of
                    // `superblocks` (SPEC 2.11.1).
                    if p.sb.is_none() {
                        return Err(Error::Corrupt(
                            "partition map merges without the feature superblocks",
                        ));
                    }
                    superblock::validate(roles, aq, g.bw, skip)?;
                }
                Ok((m, sp))
            })
            .inspect_err(|_| q.fill(0))?;
            let small = simd::Small {
                split: if split { &splits[..] } else { &[] },
                dq: p.small.as_deref().expect("small-transform tables"),
                int: p.small_int,
            };
            if merged {
                let t = p.sb.as_deref().expect("superblock tables");
                superblock::recon_stripe::<BOUNDED>(
                    &render, t, g, s, aq, roles, out, simd, q, eobs, planes, lin, small,
                );
                return Ok(());
            }
            if split {
                recon_stripe_mode::<BOUNDED>(
                    &render,
                    g,
                    s,
                    aq,
                    layout::ModeMap::Constant(0),
                    out,
                    simd,
                    q,
                    eobs,
                    planes,
                    lin,
                    small,
                );
                return Ok(());
            }
        }
    }
    let modes = p.record(g, s, bytes)?.0;
    recon_stripe_mode::<BOUNDED>(
        &render,
        g,
        s,
        aq,
        modes,
        out,
        simd,
        q,
        eobs,
        planes,
        lin,
        simd::Small::NONE,
    );
    Ok(())
}

/// Reconstructs stripe `s` from its decoded coefficients (`q`, zeroed as
/// read when `p.clear_q` is true, and `eobs`) into `out`.
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn recon_stripe_mode<const BOUNDED: bool>(
    p: &Reconstruction<'_>,
    g: &Geom,
    s: usize,
    aq: &[f32],
    modes: layout::ModeMap<'_>,
    out: &mut [&mut [u8]],
    simd: bool,
    q: &mut [i32],
    eobs: &[u8],
    planes: &mut Vec<f32>,
    lin: &mut Vec<f32>,
    small: simd::Small<'_>,
) {
    let (by0, by1) = g.rows(s);
    // Each block row is reconstructed and color converted in chunks of
    // `cw` blocks, so the planes chunk written by the IDCT is still in L1
    // when the color kernel reads it.
    let cw = if BOUNDED {
        RECON_CHUNK
    } else {
        recon_chunk_blocks()
    }
    .min(g.bw);
    let pw = cw * 8;
    // Plane row stride: padded by 64 bytes so the 8 rows a block writes do
    // not share their low 12 address bits (4K aliasing) for widths whose
    // rows are a multiple of 4 KiB.
    let ps = pw + 16;
    planes.resize(NCH * 8 * ps, 0.0);
    let float_out =
        matches!(p.info.sample, SampleType::F16 | SampleType::F32) || p.xfer16.is_some();
    lin.resize(if float_out { pw * 3 } else { 0 }, 0.0);
    let kind = InputKind::of(p.info).unwrap();
    let wide = kind == InputKind::U16;
    let xyb16 = p.xfer16.as_ref();
    let bps = p.info.sample.bytes();
    for r in 0..by1 - by0 {
        // Per-block AQ step multipliers of this block row (empty: none).
        let aqr = if aq.is_empty() {
            &[][..]
        } else {
            &aq[r * g.bw..(r + 1) * g.bw]
        };
        let y0 = (by0 + r) * 8;
        let rows = 8.min(g.h - y0);
        let skip = p.code0_skips;
        for (bx0, bx1) in coded_runs(skip, aqr, g.bw, cw) {
            let t = prof::start();
            if skip && p.clear_q {
                // Kept blocks: clear their coefficients (left all zero for
                // the next stripe).
                let end = runs_end(bx1, aqr, g.bw);
                for bx in bx1..end {
                    let b = r * g.bw + bx;
                    q[b * NCH * 64..(b + 1) * NCH * 64].fill(0);
                }
            }
            // Pixel columns x0..x0 + n of this chunk.
            let x0 = bx0 * 8;
            let n = (bx1 * 8).min(g.w) - x0;
            let rb = n * 3 * bps;
            let Err(t) = flat_run(
                p,
                g,
                by0,
                r,
                (bx0, bx1),
                q,
                eobs,
                out,
                lin,
                (kind, wide, xyb16, simd),
                t,
            ) else {
                continue;
            };
            if simd && p.layouts == [LossyChannelLayout::default(); 3] && small.split.is_empty() {
                simd::recon_block_row(
                    q,
                    eobs,
                    r,
                    g.bw,
                    bx0,
                    bx1,
                    p.dq,
                    aqr,
                    p.bias.as_deref(),
                    planes,
                    ps,
                    simd::Small::NONE,
                    p.clear_q,
                );
            } else if p.layouts == [LossyChannelLayout::default(); 3] {
                // 4x4 planes (small transforms), or the scalar route of 8x8
                // blocks: the reference.
                superblock::recon8(
                    p,
                    q,
                    eobs,
                    r,
                    g.bw,
                    (bx0, bx1),
                    aqr,
                    planes,
                    0,
                    ps,
                    simd,
                    small,
                );
            } else {
                for bx in bx0..bx1 {
                    let b = r * g.bw + bx;
                    for ch in 0..NCH {
                        let mut blk = [0.0f32; 64];
                        let qq = &mut q[(b * NCH + ch) * 64..][..64];
                        let l = p.layouts[ch].layout(modes.get(b) & (1 << ch) != 0);
                        if l == subblock::Layout::FULL8 {
                            let dqs = aq_dequant(&p.dq[ch], aqr.get(bx).copied());
                            match &p.bias {
                                // Lossy 0.4: (q - sign(q) * B) * step.
                                Some(bt) => {
                                    for i in 0..64 {
                                        let s = qq[i].clamp(-1, 1);
                                        let b = bt[ch][(qq[i] != s) as usize][i];
                                        blk[i] = (qq[i] as f32 - s as f32 * b) * dqs[i];
                                    }
                                }
                                None => {
                                    for i in 0..64 {
                                        blk[i] = qq[i] as f32 * dqs[i];
                                    }
                                }
                            }
                            dct::idct8x8(&mut blk);
                        } else {
                            let split = (modes.get(b) & (1 << ch) != 0) as usize;
                            let steps = &p.layout_steps[ch][split];
                            let aq = aqr.get(bx).copied().unwrap_or(1.0);
                            match &p.bias {
                                // Lossy 0.4: (q - sign(q) * B) * step.
                                Some(bt) => {
                                    for i in 0..64 {
                                        let s = qq[i].clamp(-1, 1);
                                        let b = bt[ch][(qq[i] != s) as usize][i];
                                        blk[i] = (qq[i] as f32 - s as f32 * b)
                                            * (steps[i] * if i == 0 { 1.0 } else { aq });
                                    }
                                }
                                None => {
                                    for i in 0..64 {
                                        blk[i] = qq[i] as f32
                                            * (steps[i] * if i == 0 { 1.0 } else { aq });
                                    }
                                }
                            }
                            blk = l.inverse(&blk);
                        }
                        qq.fill(0);
                        for y in 0..8 {
                            planes[(ch * 8 + y) * ps + (bx - bx0) * 8..][..8]
                                .copy_from_slice(&blk[y * 8..y * 8 + 8]);
                        }
                    }
                }
            }
            t.add(2);
            let t = prof::start();
            for y in 0..rows {
                let row = &mut out[y0 - by0 * 8 + y][x0 * 3 * bps..][..rb];
                let pl = |c: usize| &planes[(c * 8 + y) * ps..][..pw];
                color_chunk_row(p, kind, wide, xyb16, simd, pl(0), pl(1), pl(2), lin, row, n);
            }
            t.add(3);
        }
    }
}

/// The flat fast path of the reconstruction of run `bx0..bx1` of block row
/// `r` (stripe-local; the stripe starts at block row `by0`). When every
/// visible block of the run has `eobs == 0` in all three channels, each
/// channel plane is a per-block constant `q[DC] * dq[0]` (AQ never scales
/// DC), so every output pixel of an 8px group, in every row, is identical:
/// compute one pixel per group, expand its bytes into the run's first output
/// row, then copy it to the other rows; recon and the planes roundtrip are
/// skipped entirely. Only a fully flat run earns this: partial masks
/// fragment the wide color calls into narrow tails, which costs more than
/// the saved math (+1..3% Ir measured). The scan exits on the first
/// non-flat block. Chunks wider than 64 blocks take the ordinary path, as
/// in the qualified timing (2026-10-06). Returns the stage timer when the
/// run is not flat.
#[inline(always)]
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(in crate::lossy) fn flat_run(
    p: &Reconstruction<'_>,
    g: &Geom,
    by0: usize,
    r: usize,
    (bx0, bx1): (usize, usize),
    q: &mut [i32],
    eobs: &[u8],
    out: &mut [&mut [u8]],
    lin: &mut [f32],
    (kind, wide, xyb16, simd): (InputKind, bool, Option<&(Vec<f32>, f32)>, bool),
    t: prof::T,
) -> Result<(), prof::T> {
    let y0 = (by0 + r) * 8;
    let rows = 8.min(g.h - y0);
    let bps = p.info.sample.bytes();
    // Pixel columns x0..x0 + n of this chunk.
    let x0 = bx0 * 8;
    let n = (bx1 * 8).min(g.w) - x0;
    let rb = n * 3 * bps;
    let bpp = 3 * bps;
    let ng = n.div_ceil(8);
    let flat = p.layouts == [LossyChannelLayout::default(); 3]
        && ng <= 64
        && (bx0..bx0 + ng).all(|bx| eobs[(r * g.bw + bx) * NCH..][..NCH].iter().all(|&v| v == 0));
    if !flat {
        return Err(t);
    }
    // Expand each group's pixel into the first output row of the block
    // row; remaining rows copy it verbatim.
    let mut px = [0u8; 12];
    let r0 = y0 - by0 * 8;
    for i in 0..ng {
        let b = r * g.bw + bx0 + i;
        // Plane slices padded to a full vector: the kernels' tail path loads
        // a whole 8-wide chunk for `n < 8`.
        let (x, y, z) = (
            [q[b * NCH * 64] as f32 * p.dq[0][0]; 8],
            [q[(b * NCH + 1) * 64] as f32 * p.dq[1][0]; 8],
            [q[(b * NCH + 2) * 64] as f32 * p.dq[2][0]; 8],
        );
        color_chunk_row(
            p,
            kind,
            wide,
            xyb16,
            simd,
            &x,
            &y,
            &z,
            lin,
            &mut px[..bpp],
            1,
        );
        for j in 0..(n - i * 8).min(8) {
            out[r0][x0 * bpp + (i * 8 + j) * bpp..][..bpp].copy_from_slice(&px[..bpp]);
        }
    }
    if p.clear_q {
        // Keep the coefficient-zero invariant: recon clears each DC it
        // consumed.
        for bx in bx0..bx1 {
            let b = r * g.bw + bx;
            for ch in 0..NCH {
                q[(b * NCH + ch) * 64] = 0;
            }
        }
    }
    t.add(2);
    let t = prof::start();
    for y in 1..rows {
        let (pre, post) = out.split_at_mut(r0 + y);
        post[0][x0 * bpp..][..rb].copy_from_slice(&pre[r0][x0 * bpp..][..rb]);
    }
    t.add(3);
    Ok(())
}

/// Runs [a, b) of at most `cw` coded blocks of a block row: chunks of `cw`
/// blocks, or with `skip` the maximal runs of blocks whose code is not 0
/// (split at `cw`). The first run's leading kept blocks and each run's
/// trailing ones are cleared by the caller ([`runs_end`]).
/// Block scale 0 skips reconstruction: keep blocks (code 0, inter frames)
/// or exact blocks (code 1, written after the stripe).
pub(in crate::lossy) fn code0_skips(p: &LossyParsed<'_>) -> bool {
    p.skip || p.exact.is_some()
}

pub(in crate::lossy) fn coded_runs(
    skip: bool,
    aqr: &[f32],
    bw: usize,
    cw: usize,
) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    if !skip {
        for a in (0..bw).step_by(cw) {
            v.push((a, (a + cw).min(bw)));
        }
        return v;
    }
    let mut bx = 0;
    while bx < bw {
        if aqr[bx] == 0.0 {
            bx += 1;
            continue;
        }
        let a = bx;
        while bx < bw && aqr[bx] != 0.0 && bx - a < cw {
            bx += 1;
        }
        v.push((a, bx));
    }
    if v.is_empty() || v[0].0 > 0 {
        // Leading kept blocks: an empty run at 0 clears them.
        v.insert(0, (0, 0));
    }
    v
}

/// End of the kept blocks after a run ending at `b`.
pub(in crate::lossy) fn runs_end(b: usize, aqr: &[f32], bw: usize) -> usize {
    let mut e = b;
    while e < bw && aqr[e] == 0.0 {
        e += 1;
    }
    e
}

/// Lossy stripes per work item when one DC stripe spans `k > 1` of them:
/// all `k` (the DC stripe is decoded once) unless that leaves fewer than
/// about three items per thread, e.g. short wide images whose whole height
/// is one DC stripe; then the groups are split into smaller items, each
/// decoding its DC stripe again (a DC stripe is ~1/64 of its lossy stripes'
/// pixels). Zen 4, 12 threads: 32768x64 2.9-3.8x, 1440x900 and 11520x900
/// +16-17%, tall images unchanged (benchmarks/lossy_dc_split_zen4_2026-10-03.md).
pub(in crate::lossy) fn split_dc_groups(nstripes: usize, k: usize, threads: usize) -> usize {
    if k <= 1 || threads <= 1 {
        return k.max(1);
    }
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    match DC_SPLIT_LAB.load(core::sync::atomic::Ordering::Relaxed) {
        0 => {}
        n => return n,
    }
    if nstripes.div_ceil(k) >= 2 * threads {
        return k;
    }
    // Target items: ~3 per thread when there are enough stripes, else two
    // stripes per item (pairs share the DC decode and use the two-stripe
    // kernel), else single stripes. Each DC group is divided evenly.
    let target = if nstripes >= 6 * threads {
        nstripes / (3 * threads)
    } else if nstripes >= 2 * threads {
        2
    } else {
        1
    }
    .clamp(1, k);
    let per = k.div_ceil(k.div_ceil(target));
    if per >= 2 {
        (per + (per & 1)).min(k)
    } else {
        1
    }
}

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
static DC_SPLIT_LAB: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// DC values and AQ scales of one lossy stripe's rows of a nested DC stripe.
#[cfg(feature = "std")]
pub(in crate::lossy) type DcChunk = (Vec<i32>, Vec<f32>);

/// One lossy stripe's place in a shared DC stripe.
#[cfg(feature = "std")]
pub(in crate::lossy) enum DcSlot {
    Pending,
    Ready(DcChunk),
    Taken,
}

/// A nested DC unit (DC stripe, column group) shared by the work items of its
/// lossy stripes: decoded once, top to bottom, by the first worker that needs
/// it; each lossy stripe's rows are published as soon as they are decoded and
/// taken (moved out) by the one item that decodes that stripe.
#[cfg(feature = "std")]
pub(in crate::lossy) struct DcUnit {
    pub(in crate::lossy) state: std::sync::Mutex<DcUnitState>,
    pub(in crate::lossy) ready: std::sync::Condvar,
}

#[cfg(feature = "std")]
pub(in crate::lossy) struct DcUnitState {
    pub(in crate::lossy) claimed: bool,
    pub(in crate::lossy) slots: Vec<DcSlot>,
    pub(in crate::lossy) failed: Option<Error>,
}

/// Every DC unit of one threaded decode. Without sharing, a DC stripe split
/// over several work items (`split_dc_groups`) is decoded again by each
/// item, which made many-thread decodes of tall DC stripes up to 6x slower
/// (benchmarks/lossy_stripe_decode_2026-10-07). Shared, the items no longer
/// follow DC groups and the DC decode overlaps the lossy stripes below it.
#[cfg(feature = "std")]
pub(in crate::lossy) struct SharedDc {
    pub(in crate::lossy) units: Vec<DcUnit>,
}

#[cfg(feature = "std")]
impl SharedDc {
    /// `units` DC units of up to `k` lossy stripes each (`k` capped at the
    /// image's lossy stripes).
    pub(in crate::lossy) fn new(units: usize, k: usize) -> Result<Self, Error> {
        let mut v = Vec::new();
        v.try_reserve_exact(units).map_err(|_| Error::TooLarge)?;
        for _ in 0..units {
            let mut slots = Vec::new();
            slots.try_reserve_exact(k).map_err(|_| Error::TooLarge)?;
            slots.extend((0..k).map(|_| DcSlot::Pending));
            v.push(DcUnit {
                state: std::sync::Mutex::new(DcUnitState {
                    claimed: false,
                    slots,
                    failed: None,
                }),
                ready: std::sync::Condvar::new(),
            });
        }
        Ok(Self { units: v })
    }

    /// Lossy stripe `j` of unit `u`: taken when published, decoded here
    /// (`produce` decodes and publishes the whole unit) when no worker has
    /// started the unit, else waited for. A failed decode returns its error
    /// to every waiting item.
    pub(in crate::lossy) fn take(
        &self,
        u: usize,
        j: usize,
        produce: impl FnOnce(&Self, usize) -> Result<(), Error>,
    ) -> Result<DcChunk, Error> {
        let unit = &self.units[u];
        let mut produce = Some(produce);
        let mut st = unit.state.lock().unwrap();
        loop {
            match core::mem::replace(&mut st.slots[j], DcSlot::Taken) {
                DcSlot::Ready(chunk) => return Ok(chunk),
                DcSlot::Taken => return Err(Error::Corrupt("DC stripe rows taken twice")),
                DcSlot::Pending => st.slots[j] = DcSlot::Pending,
            }
            if let Some(e) = &st.failed {
                return Err(e.clone());
            }
            if produce.is_none() {
                // Decoded here, yet this stripe's rows were not published.
                return Err(Error::Corrupt("DC stripe rows missing"));
            }
            if !st.claimed {
                st.claimed = true;
                drop(st);
                if let Some(f) = produce.take() {
                    self.run(u, f)?;
                }
                st = unit.state.lock().unwrap();
                continue;
            }
            st = unit.ready.wait(st).unwrap();
        }
    }

    /// Decodes unit `u` unless a worker has started it.
    pub(in crate::lossy) fn produce_unit(
        &self,
        u: usize,
        produce: impl FnOnce(&Self, usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut st = self.units[u].state.lock().unwrap();
        if st.claimed {
            return Ok(());
        }
        st.claimed = true;
        drop(st);
        self.run(u, produce)
    }

    /// Runs the claimed unit's `produce`, recording its error (or panic) as
    /// the unit's failure so that no waiting item stays blocked.
    pub(in crate::lossy) fn run(
        &self,
        u: usize,
        produce: impl FnOnce(&Self, usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let mut guard = FailUnit(
            &self.units[u],
            Some(Error::Corrupt("DC stripe decode panicked")),
        );
        let r = produce(self, u);
        guard.1 = r.as_ref().err().cloned();
        r
    }

    /// Publishes lossy stripe `j`'s rows of unit `u` and wakes its waiter.
    pub(in crate::lossy) fn publish(&self, u: usize, j: usize, chunk: DcChunk) {
        let unit = &self.units[u];
        unit.state.lock().unwrap().slots[j] = DcSlot::Ready(chunk);
        unit.ready.notify_all();
    }
}

/// Records an armed error as the unit's failure on drop and wakes waiters.
#[cfg(feature = "std")]
pub(in crate::lossy) struct FailUnit<'a>(&'a DcUnit, Option<Error>);

#[cfg(feature = "std")]
impl Drop for FailUnit<'_> {
    fn drop(&mut self) {
        if let Some(e) = self.1.take() {
            let mut st = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
            st.failed = Some(e);
            drop(st);
            self.0.ready.notify_all();
        }
    }
}

/// How unbounded std decodes schedule DC stripes that span several lossy
/// stripes: 0 = each work item decodes its DC stripe (the bounded and no_std
/// path), 1 = shared DC units, decoded by the first item that needs one,
/// 2 = shared DC units decoded by their own work items (`Item::Dc`), with
/// the lossy items entropy decoding before they wait for DC rows.
#[cfg(feature = "std")]
pub(in crate::lossy) const SHARED_DC: u8 = 2;

#[cfg(feature = "std")]
pub(in crate::lossy) fn shared_dc_mode() -> u8 {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    return SHARED_DC_LAB.load(core::sync::atomic::Ordering::Relaxed);
    #[cfg(not(any(all(test, feature = "encode"), feature = "lab")))]
    SHARED_DC
}

#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
static SHARED_DC_LAB: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(SHARED_DC);

// Decodes on this thread that shared DC units (tests check coverage).
#[cfg(all(feature = "std", all(test, feature = "encode")))]
std::thread_local! {
    pub(crate) static SHARED_DC_USED: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// LAB: DC scheduling mode (see `SHARED_DC`).
#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
pub(crate) fn set_shared_dc(mode: u8) {
    SHARED_DC_LAB.store(mode, core::sync::atomic::Ordering::Relaxed);
}

/// LAB: lossy stripes per work item within a DC stripe (0 = automatic).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn set_dc_split(n: usize) {
    DC_SPLIT_LAB.store(n, core::sync::atomic::Ordering::Relaxed);
}

/// Single-stripe work items kept at the tail, per thread (pairs before
/// them): the pairs' interleaved entropy decode is faster per stripe, the
/// small tail items keep threads finishing together.
pub(in crate::lossy) const TAIL_PER_THREAD: usize = 2;

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
static PAIRS_LAB: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// LAB: stripe pairing (0 automatic, 1 always, 2 never).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn set_pairs(v: u8) {
    PAIRS_LAB.store(v, core::sync::atomic::Ordering::Relaxed);
}

/// Number of leading stripes decoded in pairs (rounded down to even).
pub(in crate::lossy) fn paired_stripes(nstripes: usize, threads: usize) -> usize {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    match PAIRS_LAB.load(core::sync::atomic::Ordering::Relaxed) {
        1 => return nstripes & !1,
        2 => return 0,
        _ => {}
    }
    let tail = if threads > 1 {
        TAIL_PER_THREAD * threads
    } else {
        0
    };
    nstripes.saturating_sub(tail) & !1
}

#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
static KEEP_SCRATCH: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
static KEPT: std::sync::Mutex<Vec<Scratch>> = std::sync::Mutex::new(Vec::new());

/// LAB: keep per-thread scratch buffers across decodes.
#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) fn set_keep_scratch(on: bool) {
    KEEP_SCRATCH.store(on, core::sync::atomic::Ordering::Relaxed);
    if !on {
        KEPT.lock().unwrap().clear();
    }
}

/// Default reconstruction chunk width in blocks (see `decode_stripe`).
pub(in crate::lossy) const RECON_CHUNK: usize = 32;

#[cfg(feature = "lab")]
pub(crate) static IDCT_FMA: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
static RECON_CHUNK_LAB: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Serialize test override writers while permitting ordinary parallel readers.
/// The guard restores the default even when an assertion unwinds.
#[cfg(all(all(test, feature = "encode"), feature = "lab", feature = "std"))]
pub(crate) fn scoped_recon_chunk_override() -> impl Drop {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct Restore {
        _guard: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            set_recon_chunk(0);
        }
    }
    let guard = LOCK.lock().unwrap();
    set_recon_chunk(0);
    Restore { _guard: guard }
}

/// LAB: overrides the reconstruction chunk width in blocks (0 = default;
/// `usize::MAX` = whole block rows, the unfused order).
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn set_recon_chunk(n: usize) {
    RECON_CHUNK_LAB.store(n, core::sync::atomic::Ordering::Relaxed);
}

pub(in crate::lossy) fn recon_chunk_blocks() -> usize {
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    {
        let n = RECON_CHUNK_LAB.load(core::sync::atomic::Ordering::Relaxed);
        if n != 0 {
            return n;
        }
    }
    RECON_CHUNK
}

/// Color-converts `n` pixels of one plane row (`p0..p2`) into `row`
/// (RGB of the output sample type).
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn color_chunk_row(
    p: &Reconstruction<'_>,
    kind: InputKind,
    wide: bool,
    xyb16: Option<&(alloc::vec::Vec<f32>, f32)>,
    simd: bool,
    p0: &[f32],
    p1: &[f32],
    p2: &[f32],
    lin: &mut [f32],
    row: &mut [u8],
    n: usize,
) {
    let pl = [p0, p1, p2];
    if kind == InputKind::U32 {
        precision::output_row(p.info, p.color, p.inv_scale, pl, xyb16, row, n);
        return;
    }
    if precision::custom_color(p.info) {
        if simd {
            let ranges = color_index(p.info.channels)
                .map(|c| crate::mask_of(p.info.channel_bits(c).unwrap() as u32) as f32);
            if let Some((table, lmax)) = xyb16 {
                let lin = &mut lin[..n * 3];
                simd::xyb_row_linear(p0, p1, p2, p.inv_scale, lin, n);
                simd::encode_range_row(lin, table, *lmax, row, ranges, p.info.sample.bytes());
            } else {
                simd::ycocg_range_row(
                    p0,
                    p1,
                    p2,
                    p.inv_scale,
                    row,
                    n,
                    ranges,
                    crate::mask_of(p.info.sample.depth()) as f32,
                    p.info.sample.bytes(),
                );
            }
        } else {
            precision::output_row(p.info, p.color, p.inv_scale, pl, xyb16, row, n);
        }
        return;
    }
    if p.xyb_power {
        match kind {
            InputKind::U8 => xyb_power::row_u8(simd, p0, p1, p2, p.inv_scale, row, n),
            InputKind::U16 => xyb_power::row_u16(simd, p0, p1, p2, p.inv_scale, row, n),
            _ => unreachable!("V4 XYB power transfer only admits U8 and U16"),
        }
        return;
    }
    if matches!(kind, InputKind::F16 | InputKind::F32) || xyb16.is_some() {
        // Linear-light output straight from the XYB inverse.
        let lin = &mut lin[..n * 3];
        if simd {
            simd::xyb_row_linear(p0, p1, p2, p.inv_scale, lin, n);
        } else {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                lin[x * 3..x * 3 + 3].copy_from_slice(&color::inverse_xyb_linear(p.inv_scale, v));
            }
        }
        if let Some((table, lmax)) = xyb16 {
            if simd {
                simd::encode16_row(lin, table, *lmax, row);
            } else {
                for (o, v) in row.as_chunks_mut::<2>().0.iter_mut().zip(lin.iter()) {
                    *o = transfer::encode16(table, *lmax, *v).to_ne_bytes();
                }
            }
        } else if kind == InputKind::F32 {
            for (o, v) in row.as_chunks_mut::<4>().0.iter_mut().zip(lin.iter()) {
                *o = v.to_ne_bytes();
            }
        } else {
            for (o, v) in row.as_chunks_mut::<2>().0.iter_mut().zip(lin.iter()) {
                *o = color::f32_to_f16_rne(*v).to_ne_bytes();
            }
        }
        return;
    }
    if let Some(m) = &p.transform {
        // Lossy 0.4 signalled linear color transform (U8 only; parse rejects
        // other sample types and precision declarations).
        if simd {
            simd::linear_row(p0, p1, p2, p.inv_scale, m, row, n);
        } else {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                let o = color::inverse_linear_px(m, p.inv_scale, v);
                row[x * 3..x * 3 + 3].copy_from_slice(&o);
            }
        }
        return;
    }
    match (simd, wide) {
        (true, false) => simd::color_row(p.color, p0, p1, p2, p.inv_scale, row, n),
        (true, true) => simd::ycocg_row16(p0, p1, p2, p.inv_scale, row, n),
        (false, false) => {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                let o = color::inverse_px(p.color, p.inv_scale, v);
                row[x * 3..x * 3 + 3].copy_from_slice(&o);
            }
        }
        (false, true) => {
            for x in 0..n {
                let v = [pl[0][x], pl[1][x], pl[2][x]];
                let o = color::inverse_ycocg16(p.inv_scale, v);
                for c in 0..3 {
                    row[x * 6 + 2 * c..x * 6 + 2 * c + 2].copy_from_slice(&o[c].to_ne_bytes());
                }
            }
        }
    }
}

/// Packs `rows` color rows (RGB of the sample type, tightly packed) and
/// optional alpha rows into the image's channel layout. Gray takes the
/// green sample (gray input decodes to R = G = B up to rounding).
pub(in crate::lossy) fn pack_rows(
    info: &ImageInfo,
    w: usize,
    rgb: &[u8],
    alpha: Option<&[u8]>,
    out: &mut [&mut [u8]],
) {
    match info.sample.bytes() {
        1 => pack_rows_typed::<1>(info.channels, w, rgb, alpha, out),
        2 => pack_rows_typed::<2>(info.channels, w, rgb, alpha, out),
        4 => pack_rows_typed::<4>(info.channels, w, rgb, alpha, out),
        _ => unreachable!("unsupported sample size"),
    }
}

pub(in crate::lossy) fn pack_rows_typed<const BPS: usize>(
    channels: Channels,
    w: usize,
    rgb: &[u8],
    alpha: Option<&[u8]>,
    out: &mut [&mut [u8]],
) {
    let nco = channels.count();
    let gray = matches!(channels, Channels::Gray | Channels::GrayAlpha);
    // Callers supply decoded alpha exactly when the image layout includes it.
    debug_assert_eq!(channels.has_alpha(), alpha.is_some());
    for (y, row) in out.iter_mut().enumerate() {
        let src = rgb[y * w * 3 * BPS..][..w * 3 * BPS]
            .as_chunks::<BPS>()
            .0
            .as_chunks::<3>()
            .0;
        let dst = row[..w * nco * BPS].as_chunks_mut::<BPS>().0;
        let arow = alpha.map(|a| a[y * w * BPS..][..w * BPS].as_chunks::<BPS>().0);
        // Samples stay byte arrays, preserving native endian and float payloads.
        // Fixed-size assignments expose the same vectorizable loops at all BPS.
        match (gray, arow) {
            (true, None) => {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = s[1];
                }
            }
            (true, Some(a)) => {
                for ((d, s), &a) in dst.as_chunks_mut::<2>().0.iter_mut().zip(src).zip(a) {
                    *d = [s[1], a];
                }
            }
            (false, Some(a)) => {
                for ((d, s), &a) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src).zip(a) {
                    *d = [s[0], s[1], s[2], a];
                }
            }
            (false, None) => dst.as_chunks_mut::<3>().0.copy_from_slice(src),
        }
    }
}

/// Decode stage timers for imij-bench (`lab` + `std`); no-ops otherwise.
/// Stages: 0 DC image, 1 entropy, 2 IDCT, 3 color.
#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) mod prof {
    use core::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub(crate) static NS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
    /// Stage timing on (default); A/B timing turns it off.
    pub(crate) static ON: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(true);
    pub(crate) struct T(Option<std::time::Instant>);
    pub(crate) fn start() -> T {
        T(ON.load(Relaxed).then(std::time::Instant::now))
    }
    impl T {
        pub(crate) fn add(self, i: usize) {
            if let Some(t) = self.0 {
                NS[i].fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
            }
        }
    }
    pub(crate) fn take() -> [u64; 6] {
        core::array::from_fn(|i| NS[i].swap(0, Relaxed))
    }
    /// Encoder token counts: tokens, lockstep slots (16 x longest lane per
    /// stripe).
    pub(crate) static TOK: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    pub(crate) fn tokens(lens: &[u32]) {
        TOK[0].fetch_add(lens.iter().map(|&v| v as u64).sum(), Relaxed);
        TOK[1].fetch_add(
            *lens.iter().max().unwrap_or(&0) as u64 * lens.len() as u64,
            Relaxed,
        );
    }
    static SIDE: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
    pub(crate) fn side_stats(s: &[u64; 3]) {
        for (a, &v) in SIDE.iter().zip(s) {
            a.store(v, Relaxed);
        }
    }
    pub(crate) fn take_side_stats() -> [u64; 3] {
        core::array::from_fn(|i| SIDE[i].load(Relaxed))
    }
    // The entropy-table section `encode_lossy_frame` wrote to its file:
    // `(entropy mode byte, table payload bytes)`, `(u8::MAX, 0)` before any
    // encode on this thread (split-bound seed-table study, 2026-10-10).
    // Thread-local: a harness that encodes on worker threads reads it on the
    // thread that ran the encode.
    std::thread_local! {
        static AC_TABLES: core::cell::Cell<(u64, u64)> =
            const { core::cell::Cell::new((u8::MAX as u64, 0)) };
    }
    pub(crate) fn ac_tables(mode: u8, bytes: u64) {
        AC_TABLES.with(|c| c.set((mode as u64, bytes)));
    }
    pub(crate) fn take_ac_tables() -> [u64; 2] {
        AC_TABLES.with(|c| {
            let v = c.get();
            c.set((u8::MAX as u64, 0));
            [v.0, v.1]
        })
    }
    pub(crate) fn take_tokens() -> [u64; 2] {
        core::array::from_fn(|i| TOK[i].swap(0, Relaxed))
    }
}
#[cfg(not(all(feature = "lab", feature = "std")))]
pub(crate) mod prof {
    pub(crate) struct T;
    #[inline(always)]
    pub(crate) fn start() -> T {
        T
    }
    impl T {
        #[inline(always)]
        pub(crate) fn add(self, _: usize) {}
    }
}

/// Forces the AVX-512 entropy kernel's lookup style (`None`: automatic).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
pub(crate) fn force_gather(on: Option<bool>) {
    let v = match on {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    entropy_avx512::FORCE.store(v, core::sync::atomic::Ordering::Relaxed);
}

/// The entropy kernel chosen for this CPU (None without AVX-512): the
/// run-level kernel's lookup style.
#[cfg(all(target_arch = "x86_64", feature = "lab"))]
pub(crate) fn kernel_choice_str() -> Option<alloc::string::String> {
    use archmage::SimdToken;
    archmage::X64V4Token::summon()
        .map(|_| alloc::format!("run-level {:?}", kernel_choice::choice().rl))
}

/// Decodes a lossy stream into `out` (rows of `stride` bytes).
/// Decodes a frame; `inter`: `out` holds the previous
/// frame, and blocks coded "keep" stay as they are.
pub(crate) fn decode_lossy_frame(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
) -> Result<ImageInfo, Error> {
    decode_lossy_impl(
        info,
        version,
        color,
        lossy,
        body,
        out,
        stride,
        threads,
        &crate::Limits::DEFAULT,
        true,
        inter,
    )
}

/// `simd = false` runs the scalar reference (tests compare the two).
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn decode_lossy_impl(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
) -> Result<ImageInfo, Error> {
    let p = parse_lossy_as(info, version, color, lossy, body)?;
    decode_lossy_parsed(p, out, stride, threads, limits, simd, inter)
}

/// Decode validated structure. Allocating callers parse before reserving the
/// output, then reuse that state rather than rebuilding nested tables.
pub(crate) fn decode_lossy_parsed(
    p: LossyParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
) -> Result<ImageInfo, Error> {
    decode_lossy_parsed_checked::<false>(p, out, stride, threads, limits, simd, inter, &|| Ok(()))
}

/// A work item of a frame decode: the lossy stripes (index, output rows) of
/// column group `.0`, or one shared DC unit (`SHARED_DC` mode 2).
pub(in crate::lossy) enum Item<'a> {
    Stripes(usize, Vec<(usize, Vec<&'a mut [u8]>)>),
    #[cfg(feature = "std")]
    Dc(usize),
}

pub(crate) fn decode_lossy_parsed_checked<const BOUNDED: bool>(
    p: LossyParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
    check: &(impl Fn() -> Result<(), Error> + Sync),
) -> Result<ImageInfo, Error> {
    decode_lossy_parsed_observed::<BOUNDED>(
        p, out, stride, threads, limits, simd, inter, check, None,
    )
}

/// [`decode_lossy_frame`] reporting each lossy unit (stripe, column group) to
/// a live `pulse` ([`crate::progress`]).
#[cfg(all(feature = "std", feature = "lab"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_lossy_frame_observed(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    pulse: &dyn how_far::Pulse,
) -> Result<ImageInfo, Error> {
    let p = parse_lossy_as(info, version, color, lossy, body)?;
    decode_lossy_parsed_observed::<false>(
        p,
        out,
        stride,
        threads,
        &crate::Limits::DEFAULT,
        true,
        inter,
        &|| Ok(()),
        Some(pulse),
    )
}

/// [`decode_lossy_parsed_checked`] reporting each lossy unit to a live
/// `pulse`: both stripes of a decoded pair start and finish together.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_lossy_parsed_observed<const BOUNDED: bool>(
    p: LossyParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    limits: &crate::Limits,
    simd: bool,
    inter: bool,
    check: &(impl Fn() -> Result<(), Error> + Sync),
    pulse: Option<&dyn how_far::Pulse>,
) -> Result<ImageInfo, Error> {
    check()?;
    if p.skip && !inter {
        return Err(Error::Corrupt(
            "keep-reference blocks outside an inter frame",
        ));
    }
    let extent = p
        .info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid output layout"))?;
    if out.len() < extent {
        return Err(Error::InvalidInput("output buffer too small"));
    }
    let rb = p.info.row_bytes();
    let rows = p.g.sb * 8;
    let nstripes = p.g.stripes();
    // Nested images (validated by `parse_lossy`): the DC image's stripes
    // span k lossy stripes, the alpha image is striped like the lossy one.
    let t = prof::start();
    let dcp = p.dc.as_ref().unwrap();
    let dc_om = crate::v1::OutMap::new(&dcp.info, &dcp.layout);
    let k = dcp.hs / p.g.sb;
    let alpha_img = p
        .alpha
        .as_ref()
        .map(|ap| (ap, crate::v1::OutMap::new(&ap.info, &ap.layout)));
    // Exact blocks: the nested image's output map and, with threads, its
    // decoded stripes shared by the work items.
    let exact_om = p
        .exact
        .as_ref()
        .map(|e| crate::v1::OutMap::new(&e.img.info, &e.img.layout));
    #[cfg(feature = "std")]
    let exact_shared = p.exact.as_ref().map(exact::Shared::new).transpose()?;
    #[cfg(not(feature = "std"))]
    let exact_shared: Option<exact::Shared> = None;
    let _ = limits;
    t.add(0);
    let rgb = p.info.channels == Channels::Rgb;
    let bps = p.info.sample.bytes();
    let pxb = rb / p.g.w;
    // Row slices of every unit (stripe, column group).
    let ng = p.ngroups();
    let mut unit_rows: Vec<Vec<&mut [u8]>> = (0..nstripes * ng).map(|_| Vec::new()).collect();
    for (y, row) in out[..extent].chunks_mut(stride).enumerate() {
        let mut rest = &mut row[..rb];
        for gi in 0..ng {
            let gw = p.group(gi).1.w;
            let (a, b) = rest.split_at_mut(gw * pxb);
            unit_rows[(y / rows) * ng + gi].push(a);
            rest = b;
        }
    }
    // Work items per column group. One DC stripe per lossy stripe (k = 1):
    // pairs of consecutive stripes whose entropy decodes run interleaved
    // (`decode_stripe_coefs2`), then single stripes at the tail so the
    // threads finish together. k > 1: the lossy stripes of one DC stripe
    // (decoded once per item), taken in pairs, unless there are too few DC
    // stripes per thread and `split_dc_groups` would split them over
    // several items, each decoding the DC stripe again. Unbounded std
    // decodes on more than one worker then share DC units instead: the
    // k = 1 items, each stripe taking its DC rows from the shared unit,
    // and (mode 2) each unit decoded by its own item queued just before the
    // unit's first stripe. Without a split, sharing gains nothing and cost
    // up to 2.5% (benchmarks/lossy_dc_schedule_2026-10-07).
    // Threads per column group; one on hosts without threads, where split
    // DC stripes would only be decoded again in turn.
    let workers = crate::par::worker_count(threads, nstripes * ng);
    // Reports each unit to a live pulse; otherwise one branch per pair.
    let observed = pulse.and_then(|pulse| {
        let (h, rows_per) = (p.info.height as usize, p.g.sb * 8);
        let shapes = (0..nstripes * ng).map(|u| {
            let (si, gi) = (u / ng, u % ng);
            let (x, gg) = p.group(gi);
            let y = si * rows_per;
            crate::progress::UnitShape {
                stripe: si,
                group: gi,
                groups: ng,
                y,
                rows: rows_per.min(h - y),
                x,
                cols: gg.w,
            }
        });
        crate::progress::UnitProgress::plan(pulse, shapes, workers)
    });
    let observed = observed.as_ref();
    let tg = if workers > 1 { threads.div_ceil(ng) } else { 1 };
    #[cfg(feature = "std")]
    let mode = if p.model_refs.is_none()
        && p.dc_coverage.is_none()
        && !BOUNDED
        && k > 1
        && workers > 1
        && split_dc_groups(nstripes, k, tg) < k
    {
        shared_dc_mode()
    } else {
        0
    };
    #[cfg(feature = "std")]
    let shared = if mode != 0 {
        #[cfg(all(test, feature = "encode"))]
        SHARED_DC_USED.with(|n| n.set(n.get() + 1));
        // A DC stripe may declare more rows than the image has: slots only
        // for the lossy stripes that exist.
        Some(SharedDc::new(nstripes.div_ceil(k) * ng, k.min(nstripes))?)
    } else {
        None
    };
    #[cfg(feature = "std")]
    let (k_items, late) = (if mode != 0 { 1 } else { k }, mode == 2);
    #[cfg(not(feature = "std"))]
    let k_items = k;
    let paired = paired_stripes(nstripes, tg);
    let per = split_dc_groups(nstripes, k_items, tg);
    let mut items: Vec<Item<'_>> = Vec::new();
    for gi in 0..ng {
        let first = items.len();
        for s in 0..nstripes {
            let r = core::mem::take(&mut unit_rows[s * ng + gi]);
            #[cfg(feature = "std")]
            if late && s % k == 0 {
                items.push(Item::Dc(s / k * ng + gi));
            }
            // The last item belongs to this group.
            let own = items.len() > first;
            if p.dc_coverage.is_some() {
                match items.last_mut() {
                    Some(Item::Stripes(_, group))
                        if own && p.dc_span(group[0].0, gi).unit == p.dc_span(s, gi).unit =>
                    {
                        group.push((s, r))
                    }
                    _ => items.push(Item::Stripes(gi, alloc::vec![(s, r)])),
                }
                continue;
            }
            match items.last_mut() {
                Some(Item::Stripes(_, g)) if own && k_items == 1 && s < paired && g.len() == 1 => {
                    g.push((s, r))
                }
                Some(Item::Stripes(_, g)) if own && k_items > 1 && s % k != 0 && g.len() < per => {
                    g.push((s, r))
                }
                _ => items.push(Item::Stripes(gi, alloc::vec![(s, r)])),
            }
        }
    }
    #[cfg(feature = "std")]
    let pool = std::sync::Mutex::new(Vec::<Scratch>::new());
    // LAB: scratch kept across decodes (page-fault experiment).
    #[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
    let pool = if !BOUNDED && KEEP_SCRATCH.load(core::sync::atomic::Ordering::Relaxed) {
        &KEPT
    } else {
        &pool
    };
    // Decodes shared DC unit `u` (DC stripe `u / ng`, column group `u % ng`)
    // top to bottom, publishing each lossy stripe's DC values as soon as its
    // rows are decoded so that items below can start before the unit ends.
    #[cfg(feature = "std")]
    let produce = |sh: &SharedDc, u: usize, px: &mut Vec<u8>| -> Result<(), Error> {
        let (d, gi) = (u / ng, u % ng);
        let dr = p.group(gi).1.bw * dcp.bpp();
        let mut rows = crate::v1::UnitRows::new(dcp, u)?;
        for s in d * k..(d * k + k).min(nstripes) {
            let nb = p.g.sb.min(p.g.bh - s * p.g.sb);
            resize_fallible(px, nb * dr, 0)?;
            for r in 0..nb {
                check()?;
                rows.next_row(dcp, &dc_om, &mut px[r * dr..][..dr])?;
            }
            let (mut dc, mut aq) = (Vec::new(), Vec::new());
            dc_values(
                dcp.info.sample,
                p.aq,
                p.exact.is_some(),
                &px[..nb * dr],
                &mut dc,
                &mut aq,
            )?;
            sh.publish(u, s - d * k, (dc, aq));
        }
        Ok(())
    };
    let decode_item = |_, item: Item<'_>| {
        check()?;
        let mut exact_st = exact::ItemState::default();
        #[cfg(feature = "std")]
        let mut sc = pool.lock().unwrap().pop().unwrap_or_default();
        #[cfg(feature = "std")]
        let (gi, mut group) = match item {
            Item::Stripes(gi, group) => (gi, group),
            Item::Dc(u) => {
                let r = match &shared {
                    Some(sh) => sh.produce_unit(u, |sh, u| produce(sh, u, &mut sc.dc_px)),
                    None => Ok(()),
                };
                pool.lock().unwrap().push(sc);
                return r;
            }
        };
        #[cfg(not(feature = "std"))]
        let Item::Stripes(gi, mut group) = item;
        #[cfg(not(feature = "std"))]
        let mut sc = Scratch::default();
        let mut dcs = [
            (core::mem::take(&mut sc.dc), core::mem::take(&mut sc.aq)),
            (core::mem::take(&mut sc.dc2), core::mem::take(&mut sc.aq2)),
        ];
        // Per-item DC: the DC stripe being decoded (index, row decoder, next
        // row); each lossy stripe decodes just its own rows when it needs
        // them, so no whole DC stripe is ever buffered.
        let mut cursor: Option<(
            usize,
            crate::v1::UnitRows<'_>,
            usize,
            Option<(
                alloc::sync::Arc<crate::v1::Parsed<'static>>,
                crate::v1::OutMap,
            )>,
        )> = None;
        let gg = p.group(gi).1;
        let r = (|| {
            let bw = gg.bw;
            for pair in group.chunks_mut(2) {
                check()?;
                if let Some(observed) = observed {
                    for (j, (s, _)) in pair.iter().enumerate() {
                        if let Err(e) = observed.begin(s * ng + gi) {
                            // A stop at a unit's checkpoint fails the pair before
                            // decoding; the units begun so far record it.
                            for (s, _) in &pair[..=j] {
                                observed.end(s * ng + gi, &Err(e.clone()));
                            }
                            return Err(e);
                        }
                    }
                }
                // DC values of each stripe of the pair into `dcs[j]`.
                let t = prof::start();
                for (j, (s, _)) in pair.iter().enumerate() {
                    #[cfg(feature = "std")]
                    if let Some(sh) = &shared {
                        // Late: taken after the entropy decode below.
                        if !late {
                            let u = *s / k * ng + gi;
                            dcs[j] = sh.take(u, *s % k, |sh, u| produce(sh, u, &mut sc.dc_px))?;
                        }
                        continue;
                    }
                    let span = p.dc_span(*s, gi);
                    let d = span.unit;
                    if cursor.as_ref().is_none_or(|c| c.0 != d) {
                        let (model, body) = if let Some(models) = &p.model_refs {
                            models.dc(dcp.stripes[d], dcp)?
                        } else {
                            (None, dcp.stripes[d])
                        };
                        let selected = model.as_deref().unwrap_or(dcp);
                        let rows = crate::v1::UnitRows::new_with(selected, d, body)?;
                        let alternate = model.map(|p| {
                            let om = crate::v1::OutMap::new(&p.info, &p.layout);
                            (p, om)
                        });
                        cursor = Some((d, rows, 0, alternate));
                    }
                    let Some((_, rows, next, alternate)) = cursor.as_mut() else {
                        unreachable!()
                    };
                    let dr = bw * dcp.bpp();
                    let nb = p.g.sb.min(p.g.bh - *s * p.g.sb);
                    let first = (*s - span.start) * p.g.sb;
                    if *next > first {
                        return Err(Error::Corrupt("DC rows out of order"));
                    }
                    resize_fallible(&mut sc.dc_px, nb * dr, 0)?;
                    // An item that starts inside a DC stripe (only when the
                    // lab switch splits DC stripes without sharing) decodes
                    // the rows above its first stripe and drops them.
                    while *next < first + nb {
                        check()?;
                        let r = next.saturating_sub(first);
                        let (model, om) = alternate
                            .as_ref()
                            .map_or((dcp, &dc_om), |(p, om)| (p.as_ref(), om));
                        rows.next_row(model, om, &mut sc.dc_px[r * dr..][..dr])?;
                        *next += 1;
                    }
                    let (dc, aq) = &mut dcs[j];
                    dc.clear();
                    aq.clear();
                    dc_values(
                        dcp.info.sample,
                        p.aq,
                        p.exact.is_some(),
                        &sc.dc_px[..nb * dr],
                        dc,
                        aq,
                    )?;
                }
                t.add(0);
                let t = prof::start();
                #[cfg(feature = "std")]
                let skip = late && shared.is_some();
                #[cfg(not(feature = "std"))]
                let skip = false;
                // Empty: the DCs are filled in after the entropy decode.
                let dc_of = |j: usize| {
                    if skip { &[][..] } else { &dcs[j].0[..] }
                };
                if pair.len() == 2 {
                    let s = [pair[0].0, pair[1].0];
                    let u = s.map(|s| s * ng + gi);
                    decode_stripe_coefs2(&p, &gg, s, u, [dc_of(0), dc_of(1)], simd, &mut sc)?;
                } else {
                    let s = pair[0].0;
                    decode_stripe_coefs(&p, &gg, s, s * ng + gi, dc_of(0), simd, &mut sc)?;
                }
                t.add(1);
                // Late DC: wait for the shared unit's rows only now, so the
                // entropy decode above overlaps the unit's decode.
                #[cfg(feature = "std")]
                if let Some(sh) = &shared
                    && late
                {
                    let t = prof::start();
                    for (j, (s, _)) in pair.iter().enumerate() {
                        let u = *s / k * ng + gi;
                        dcs[j] = sh.take(u, *s % k, |sh, u| produce(sh, u, &mut sc.dc_px))?;
                        let (by0, by1) = gg.rows(*s);
                        let q = if j == 0 { &mut sc.q } else { &mut sc.q2 };
                        fill_dc(q, &dcs[j].0, by1 - by0, bw);
                    }
                    t.add(0);
                }
                let Scratch {
                    q,
                    eobs,
                    q2,
                    eobs2,
                    planes,
                    lin,
                    alpha,
                    tmp,
                    roles,
                    splits,
                    ..
                } = &mut sc;
                for (j, (s, rws)) in pair.iter_mut().enumerate() {
                    let s = *s;
                    let (q, eobs) = if j == 0 {
                        (&mut *q, &*eobs)
                    } else {
                        (&mut *q2, &*eobs2)
                    };
                    let aq = &dcs[j].1[..];
                    let rec = p.stripes[s * ng + gi];
                    let ws = (&mut *planes, &mut *lin, &mut *roles, &mut *splits);
                    if rgb {
                        recon_unit::<BOUNDED>(&p, &gg, s, rec, aq, rws, simd, q, eobs, ws)?;
                        if let (Some(e), Some(om)) = (&p.exact, &exact_om) {
                            let shared = exact_shared.as_ref();
                            exact::apply_unit(
                                e,
                                exact::Nested::Decode { om, shared },
                                &mut exact_st,
                                e.range(s * ng + gi)?,
                                s,
                                &gg,
                                aq,
                                rws.as_mut_slice(),
                                check,
                            )?;
                        }
                    } else {
                        let n = rws.len();
                        let crb = gg.w * 3 * bps;
                        let arb = gg.w * bps;
                        resize_fallible(tmp, n.checked_mul(crb).ok_or(Error::TooLarge)?, 0)?;
                        let mut trows: Vec<&mut [u8]> = tmp.chunks_mut(crb).collect();
                        recon_unit::<BOUNDED>(&p, &gg, s, rec, aq, &mut trows, simd, q, eobs, ws)?;
                        if let (Some(e), Some(om)) = (&p.exact, &exact_om) {
                            let shared = exact_shared.as_ref();
                            exact::apply_unit(
                                e,
                                exact::Nested::Decode { om, shared },
                                &mut exact_st,
                                e.range(s * ng + gi)?,
                                s,
                                &gg,
                                aq,
                                trows.as_mut_slice(),
                                check,
                            )?;
                        }
                        let a = match &alpha_img {
                            Some((ap, om)) => {
                                let mut a = core::mem::take(alpha);
                                resize_fallible(
                                    &mut a,
                                    n.checked_mul(arb).ok_or(Error::TooLarge)?,
                                    0,
                                )?;
                                crate::v1::decode_unit_into_checked(
                                    ap,
                                    s * ng + gi,
                                    om,
                                    &mut a,
                                    arb,
                                    check,
                                )?;
                                Some(a)
                            }
                            None => None,
                        };
                        pack_rows(&p.info, gg.w, tmp, a.as_deref(), rws);
                        if let Some(a) = a {
                            *alpha = a;
                        }
                    }
                }
                if let Some(observed) = observed {
                    for (s, _) in pair.iter() {
                        observed.end(s * ng + gi, &Ok(()));
                    }
                }
            }
            Ok(())
        })();
        let [(dc, aq), (dc2, aq2)] = dcs;
        (sc.dc, sc.aq, sc.dc2, sc.aq2) = (dc, aq, dc2, aq2);
        #[cfg(feature = "std")]
        pool.lock().unwrap().push(sc);
        r
    };
    if BOUNDED {
        for (index, item) in items.into_iter().enumerate() {
            decode_item(index, item)?;
        }
    } else {
        for result in crate::par::par_map(items, threads, decode_item) {
            result?;
        }
    }
    Ok(p.info)
}

/// Shared transfer lookup allocation, separate from per-worker row scratch.
pub(crate) fn decoder_transfer_table_bytes(info: &ImageInfo, color: LossyColor) -> u128 {
    let uses_table = color == LossyColor::Xyb
        && transfer::Transfer::from_cicp(info.cicp.transfer_characteristics).is_some()
        && (matches!(info.sample, SampleType::U16 | SampleType::U32)
            || (info.sample == SampleType::U8 && precision::custom_color(info)));
    if uses_table {
        (transfer::TABLE_N as u128 + 1) * core::mem::size_of::<f32>() as u128
    } else {
        0
    }
}
