//! Pass-1 transform/quantization, optional lab post-pass-1 transforms, and
//! exact-block overlay selection.

use alloc::vec;
use alloc::vec::Vec;
use how_far::{ProgressExt, Pulse, Report, Stop};

use super::setup::{FrameSetup, RowFiller};
#[cfg(all(feature = "lab", feature = "std"))]
use crate::lossy::block_lab;
#[cfg(feature = "lab")]
use crate::lossy::lf_pred_lab;
use crate::lossy::{
    BiasSums, Coded, LossyChannelLayout, LossyConfig, NCH, StripeCoefs, aq_on_cfg, dc_row,
    enc_simd, exact, layout, lossy_group_cols, subblock, transfer,
};
use crate::{Error, ImageInfo};

/// Pass 1: transform + quantize every stripe, one block row at a time:
/// fill planar rows (linear light for XYB, code values for YCoCg), color
/// convert, then DCT + quantize.
#[allow(clippy::too_many_arguments)]
pub(super) fn quantize_pass1_stripes(
    pixels: &[u8],
    stride: usize,
    previous: Option<(&[u8], usize)>,
    cfg: &LossyConfig,
    setup: &FrameSetup,
    map: Option<&[u8]>,
    filler: &RowFiller<'_>,
    pulse: &dyn Pulse,
) -> Result<Vec<(StripeCoefs, BiasSums)>, Error> {
    let pulse = pulse.live();
    let g = &setup.g;
    crate::par::par_map(
        (0..g.stripes()).collect(),
        cfg.threads,
        |_, s| -> Result<_, Error> {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let (by0, by1) = g.rows(s);
            let nby = by1 - by0;
            let mut q = vec![0i32; nby * g.bw * NCH * 64];
            let mut nz = vec![0u64; nby * g.bw * NCH];
            let mut modes = vec![
                0u8;
                if setup.adaptive_mask != 0 {
                    nby * g.bw
                } else {
                    0
                }
            ];
            let mut planes = vec![0.0f32; NCH * 8 * setup.pw];
            let mut extremes: Vec<(u64, bool)> = Vec::with_capacity(g.bw);
            let mut coefs = vec![0.0f32; g.bw * 64];
            let mut bias_stats = BiasSums::default();
            let activity_on = cfg.aq_gamma != 0.0;
            let aq_on = activity_on || map.is_some();
            // Block codes (DC image channel 3): AQ multiplier a / 64, or 0 =
            // keep the reference block (inter frames).
            let codes_on = aq_on || setup.skip_on;
            let mut aq_codes = vec![64u8; if codes_on { nby * g.bw } else { 0 }];
            let mut aq_row = vec![1.0f32; if aq_on { g.bw } else { 0 }];
            for r in 0..nby {
                pulse.check().map_err(|_| Error::Cancelled)?;
                if let Some(m) = map {
                    let codes = &m[(by0 + r) * g.bw..][..g.bw];
                    aq_codes[r * g.bw..][..g.bw].copy_from_slice(codes);
                    for (s_aq, &a) in aq_row.iter_mut().zip(codes) {
                        *s_aq = a as f32 * (1.0 / 64.0);
                    }
                }
                filler.fill_block_row(by0 + r, &mut planes, &mut extremes);
                let qr = &mut q[r * g.bw * NCH * 64..][..g.bw * NCH * 64];
                let nr = &mut nz[r * g.bw * NCH..][..g.bw * NCH];
                for ch in 0..NCH {
                    if setup.layouts[ch].layout(false) == subblock::Layout::FULL8
                        || (activity_on && ch == 0)
                    {
                        enc_simd::fdct_row(
                            &planes[ch * 8 * setup.pw..][..8 * setup.pw],
                            setup.pw,
                            g.bw,
                            &setup.cm,
                            &mut coefs,
                        );
                    }
                    if activity_on && ch == 0 {
                        // Activity: mean |AC| of luma relative to the first AC step.
                        let r0 = cfg.aq_r0 * setup.steps[0][1];
                        for bx in 0..g.bw {
                            let c = &coefs[bx * 64..][..64];
                            let act = c[1..].iter().map(|v| v.abs()).sum::<f32>() / 63.0;
                            let m = transfer::pow((act / r0).max(1e-6) as f64, cfg.aq_gamma as f64)
                                .clamp(0.5, 2.0);
                            let a = (m * 64.0 + 0.5) as u8; // m in [0.5, 2]: round half up
                            aq_codes[r * g.bw + bx] = a;
                            aq_row[bx] = a as f32 * (1.0 / 64.0);
                        }
                    }
                    if setup.layouts[ch] == LossyChannelLayout::default() {
                        enc_simd::quant_row(
                            &coefs,
                            g.bw,
                            ch,
                            &setup.inv_steps[ch],
                            &aq_row,
                            setup.round,
                            qr,
                            nr,
                        );
                        if let Some(k) = setup.centroid_k {
                            bias_stats.add_row(
                                &coefs,
                                qr,
                                g.bw,
                                ch,
                                &setup.inv_steps[ch],
                                &aq_row,
                                k,
                            );
                        }
                        if setup.dc_fix {
                            let ext =
                                (ch == 0 && setup.white_code.is_some()).then_some(&extremes[..]);
                            dc_row(&coefs, g.bw, ch, setup.inv_steps[ch][0], ext, qr, nr);
                        }
                    } else {
                        let mr = if modes.is_empty() {
                            &mut [][..]
                        } else {
                            &mut modes[r * g.bw..(r + 1) * g.bw]
                        };
                        layout::encode_row(
                            &planes[ch * 8 * setup.pw..][..8 * setup.pw],
                            setup.pw,
                            g.bw,
                            ch,
                            setup.layouts[ch],
                            &setup.layout_steps[ch],
                            setup.steps[ch][1],
                            &coefs,
                            &aq_row,
                            setup.round,
                            if setup.dc_fix { 0.5 } else { setup.round },
                            qr,
                            nr,
                            mr,
                        );
                    }
                }
                // Inter frames: blocks whose pixels equal the previous frame's
                // keep the decoder's canvas (coded "keep"; the DC stays for a
                // smooth DC image, the AC goes).
                if let Some((pp, ps)) = previous {
                    let y0 = (by0 + r) * 8;
                    let y1 = (y0 + 8).min(g.h);
                    for bx in 0..g.bw {
                        let (x0, x1) = (
                            bx * 8 * setup.nci * setup.bps,
                            ((bx * 8 + 8).min(g.w)) * setup.nci * setup.bps,
                        );
                        let same = (y0..y1).all(|y| {
                            pixels[y * stride + x0..y * stride + x1] == pp[y * ps + x0..y * ps + x1]
                        });
                        if same {
                            for ch in 0..NCH {
                                let o = (bx * NCH + ch) * 64;
                                qr[o + 1..o + 64].fill(0);
                                nr[bx * NCH + ch] = (qr[o] != 0) as u64;
                            }
                            aq_codes[r * g.bw + bx] = 0;
                        }
                    }
                }
                pulse.advance(1);
                pulse.check().map_err(|_| Error::Cancelled)?;
            }
            Ok((
                StripeCoefs {
                    by0,
                    nby,
                    bw: g.bw,
                    q,
                    nz,
                    aq: aq_codes,
                    modes,
                    splits: Vec::new(),
                },
                bias_stats,
            ))
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, Error>>()
}

/// Applies optional lab centroid reconstruction bias, masked AC zeroing, and
/// LF prediction to pass-1 stripes.
pub(super) fn apply_lab_post_pass1(
    #[cfg_attr(not(feature = "lab"), allow(unused_mut))] mut stripes: Vec<StripeCoefs>,
    bias_stats: Vec<BiasSums>,
    cfg: &LossyConfig,
    setup: &mut FrameSetup,
) -> Vec<StripeCoefs> {
    #[cfg(feature = "lab")]
    {
        if let Some(k) = setup.centroid_k {
            let b = bias_stats.iter().fold(BiasSums::default(), |mut a, s| {
                for c in 0..3 {
                    for band in 0..2 {
                        for g in 0..2 {
                            a.sum[c][band][g] += s.sum[c][band][g];
                            a.count[c][band][g] += s.count[c][band][g];
                        }
                    }
                }
                a
            });
            let b = b.bias(k);
            setup.qp.bias = (!b.is_zero()).then_some(b);
        }
        #[cfg(feature = "std")]
        block_lab::zero_masked_ac(&mut stripes);
        if cfg.lf_pred != 0 && !aq_on_cfg(cfg) {
            lf_pred_lab(
                &mut stripes,
                &setup.g,
                &setup.steps,
                cfg.lf_pred,
                (setup.sb * (8 / setup.sb).max(1)) as usize,
            );
        }
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = (&bias_stats, cfg, &setup);
    }
    stripes
}

/// Exact blocks (`exact.rs`): blocks whose estimated lossless bits are at
/// most the policy's ratio of their run-level AC bits (within its
/// decode-cost budget) drop their AC and are coded exactly in a nested
/// lossless image. Without a qualifying block nothing changes.
pub(super) fn select_exact_blocks(
    mut stripes: Vec<StripeCoefs>,
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &LossyConfig,
    setup: &FrameSetup,
) -> Result<(Vec<StripeCoefs>, Option<Coded>), Error> {
    let eligible =
        !aq_on_cfg(cfg) && !setup.skip_on && setup.layouts == [LossyChannelLayout::default(); 3];
    let exact = match exact::policy(cfg, info, eligible) {
        Some(pol) => {
            let bits = exact::estimate_bits(&stripes, cfg.threads);
            let mask = exact::select_mask(
                pixels,
                stride,
                setup.nci * setup.bps,
                setup.g.w,
                setup.g.h,
                &bits,
                &pol,
                cfg.threads,
            );
            if mask.contains(&true) {
                exact::mark(&mut stripes, &mask);
                Some(exact::encode(
                    pixels,
                    stride,
                    setup.nci * setup.bps,
                    &setup.g,
                    lossy_group_cols(cfg, setup.g.w),
                    &mask,
                    pol.stripe_rows,
                    cfg.effort,
                    cfg.threads,
                )?)
            } else {
                None
            }
        }
        None => None,
    };
    Ok((stripes, exact))
}
