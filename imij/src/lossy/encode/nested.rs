//! Nested lossless DC image and optional alpha image encoding for whole-image
//! lossy frames.

use alloc::vec;
use alloc::vec::Vec;
use how_far::Pulse;

use crate::lossy::{
    Coded, Geom, LossyConfig, NCH, StripeCoefs, aq_on_cfg, dc_coverage, dc_lanes, dc_stripe_rows,
    seed_bank, seed_exact,
};
use crate::{Channels, Error, ImageInfo, SampleType};

/// Coded nested DC image and its coverage/stripe geometry.
pub(super) struct EncodedDc {
    pub fits16: bool,
    pub head: Vec<u8>,
    pub stripes: Vec<Vec<u8>>,
    pub explicit: Option<Coded>,
    pub dc_rows: usize,
    pub coverage_head: Option<Vec<u8>>,
    pub coverage: Option<dc_coverage::Coverage>,
    pub dc_info: ImageInfo,
}

/// Encodes the nested lossless DC image (with optional 4th channel for AQ,
/// inter keep, or exact-block codes).
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_dc_image(
    stripes: &[StripeCoefs],
    info: &ImageInfo,
    cfg: &LossyConfig,
    g: &Geom,
    sb: u32,
    gcols: usize,
    ng: usize,
    skip_on: bool,
    has_exact: bool,
    nested: &dyn Pulse,
    pulse: &dyn Pulse,
) -> Result<EncodedDc, Error> {
    // DC image: U16 (value + 32768) when every DC fits, else I32.
    let fits16 = stripes.iter().all(|sc| {
        sc.q.as_chunks::<64>()
            .0
            .iter()
            .all(|b| (-32768..=32767).contains(&b[0]))
    });
    let dcs = if fits16 { 2 } else { 4 };
    let aq_on = aq_on_cfg(cfg);
    let dch = if aq_on || skip_on || has_exact {
        4
    } else {
        NCH
    };
    let mut dc = vec![0u8; g.bw * g.bh * dch * dcs];
    for sc in stripes {
        for r in 0..sc.nby {
            pulse.check().map_err(|_| Error::Cancelled)?;
            for bx in 0..g.bw {
                let b = r * g.bw + bx;
                let o0 = (((sc.by0 + r) * g.bw + bx) * dch) * dcs;
                let put = |dc: &mut Vec<u8>, c: usize, v: i32| {
                    let o = o0 + c * dcs;
                    if fits16 {
                        dc[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                    } else {
                        dc[o..o + 4].copy_from_slice(&v.to_ne_bytes());
                    }
                };
                for c in 0..NCH {
                    let v = sc.q[(b * NCH + c) * 64];
                    put(&mut dc, c, if fits16 { v + 32768 } else { v });
                }
                if dch == 4 {
                    put(&mut dc, 3, sc.aq[b] as i32);
                }
            }
        }
    }
    let dc_info = ImageInfo::new(
        g.bw as u32,
        g.bh as u32,
        if dch == 4 {
            Channels::Rgba
        } else {
            Channels::Rgb
        },
        if fits16 {
            SampleType::U16
        } else {
            SampleType::I32
        },
    );
    let mut dc_rows = dc_stripe_rows(cfg, sb, info.width, info.height);
    let coverage_head = cfg
        .dc_coverage
        .as_ref()
        .map(|lists| dc_coverage::Coverage::write(lists));
    let coverage = coverage_head
        .as_ref()
        .map(|head| {
            dc_coverage::Coverage::read(
                head,
                g.stripes(),
                ng,
                g.sb,
                g.bh,
                &crate::parse::limits::Unrestricted,
            )
        })
        .transpose()?;
    if let Some(c) = &coverage {
        dc_rows = c
            .rectangles
            .iter()
            .map(|&(a, b, _)| b - a)
            .max()
            .unwrap()
            .div_ceil(g.sb)
            * g.sb;
    }
    let dc_cfg = crate::EncodeConfig {
        effort: cfg.effort,
        stripe_rows: Some(dc_rows as u32),
        lanes: dc_lanes(cfg),
        threads: cfg.threads,
        column_groups: Some((gcols / 8) as u32),
        // Nested images stay within Lossless (and Extended) 0.1.
        float_shift: false,
        ..Default::default()
    };
    let (head, dc_stripes, explicit) = crate::seeds::with_role(crate::seeds::Role::Dc, || {
        let seeds = seed_bank(cfg).map(|b| (b, seed_exact(cfg)));
        if let Some(c) = &coverage {
            return crate::v1::encode_dc_rectangles(
                &dc,
                g.bw * dch * dcs,
                &dc_info,
                &dc_cfg,
                seeds,
                &c.rectangles,
                nested,
            );
        }
        crate::v1::encode_parts_seeded_with_progress(
            &dc,
            g.bw * dch * dcs,
            &dc_info,
            &dc_cfg,
            seeds,
            nested,
        )
    })?;
    Ok(EncodedDc {
        fits16,
        head,
        stripes: dc_stripes,
        explicit,
        dc_rows,
        coverage_head,
        coverage,
        dc_info,
    })
}

/// Encodes the optional nested lossless gray alpha image (one stripe per lossy
/// stripe).
#[allow(clippy::too_many_arguments)]
pub(super) fn encode_alpha_image(
    info: &ImageInfo,
    cfg: &LossyConfig,
    pixels: Option<(&[u8], usize)>,
    g: &Geom,
    sb: u32,
    gcols: usize,
    nested: &dyn Pulse,
    pulse: &dyn Pulse,
) -> Result<Option<(ImageInfo, Vec<u8>, Vec<Vec<u8>>, Option<Coded>)>, Error> {
    if !info.channels.has_alpha() {
        return Ok(None);
    }
    let nci = info.channels.count();
    let bps = info.sample.bytes();
    let rb = info.row_bytes();
    let mut a = Vec::with_capacity(g.w * g.h * bps);
    for y in 0..g.h {
        pulse.check().map_err(|_| Error::Cancelled)?;
        let (pixels, stride) = pixels.expect("alpha channel requires pixel input");
        let row = &pixels[y * stride..][..rb];
        for x in 0..g.w {
            let o = (x * nci + nci - 1) * bps;
            a.extend_from_slice(&row[o..o + bps]);
        }
    }
    let ai = ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
        .with_significant_bits(&[info.channel_bits(info.channels.count() - 1).unwrap()])?;
    let acfg = crate::EncodeConfig {
        effort: cfg.effort,
        stripe_rows: Some(sb * 8),
        threads: cfg.threads,
        column_groups: Some(gcols as u32),
        // Nested images stay within Lossless (and Extended) 0.1.
        float_shift: false,
        ..Default::default()
    };
    let (ah, ar, ae) = crate::seeds::with_role(crate::seeds::Role::Alpha, || {
        let seeds = seed_bank(cfg).map(|b| (b, seed_exact(cfg)));
        crate::v1::encode_parts_seeded_with_progress(&a, g.w * bps, &ai, &acfg, seeds, nested)
    })?;
    Ok(Some((ai, ah, ar, ae)))
}
