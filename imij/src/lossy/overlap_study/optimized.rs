//! Bounded chunks, retained band scratch and flat-run colour shortcuts.
use super::*;

pub(super) fn decode_chunked(
    file: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    out: &mut [u8],
    mode: Mode,
    threads: usize,
    chunk: usize,
) -> Result<(), Error> {
    decode_inner::<false>(file, w, h, stride, out, mode, threads, chunk)
}

pub(super) fn decode_pruned(
    file: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    out: &mut [u8],
    mode: Mode,
    threads: usize,
    chunk: usize,
) -> Result<(), Error> {
    decode_inner::<true>(file, w, h, stride, out, mode, threads, chunk)
}

fn decode_inner<const PRUNE: bool>(
    file: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    out: &mut [u8],
    mode: Mode,
    threads: usize,
    chunk: usize,
) -> Result<(), Error> {
    assert!([8, 16, 32, 64].contains(&chunk));
    let extent = (h - 1) * stride + w * 3;
    assert!(stride >= w * 3 && out.len() >= extent);
    let p = parse_lossy(file)?;
    admitted(&p);
    let step = mode.step();
    let bw = p.g.bw;
    let ns = p.g.stripes();
    assert_eq!((bw, p.g.bh), (w.div_ceil(step), h.div_ceil(step)));
    let workers = crate::par::worker_count(threads, ns);
    let mut rest = &mut out[..extent];
    let mut bands = Vec::new();
    let mut emitted = 0;
    for worker in 0..workers {
        let a = worker * ns / workers;
        let b = (worker + 1) * ns / workers;
        let end = if b == ns {
            extent
        } else {
            b * p.g.sb * step * stride
        };
        let (rows, tail) = rest.split_at_mut(end - emitted);
        rest = tail;
        emitted = end;
        bands.push((a * p.g.sb, (b * p.g.sb).min(p.g.bh), rows));
    }
    let results = crate::par::par_map(bands, threads, |_, (first, last, dst)| {
        decode_band::<PRUNE, false, _>(
            &p,
            w,
            h,
            stride,
            first,
            last,
            dst,
            mode,
            chunk,
            &mut |_, _| true,
        )?;
        Ok(())
    });
    for result in results {
        result?;
    }
    Ok(())
}

#[inline]
fn row_offset<const STREAM: bool>(by: usize, first: usize, step: usize, y: usize) -> usize {
    if STREAM { y } else { (by - first) * step + y }
}

/// Delivers complete tight rows in order, using one block-row RGB buffer.
/// A false callback result explicitly requests prefix-only decoding.
pub(super) fn decode_stream(
    file: &[u8],
    w: usize,
    h: usize,
    mode: Mode,
    chunk: usize,
    mut emit: impl FnMut(usize, &[u8]) -> bool,
) -> Result<(), Error> {
    assert!([8, 16, 32, 64].contains(&chunk));
    let p = parse_lossy(file)?;
    admitted(&p);
    assert_eq!(
        (p.g.bw, p.g.bh),
        (w.div_ceil(mode.step()), h.div_ceil(mode.step()))
    );
    let mut rgb = vec![0; mode.step() * w * 3];
    decode_band::<true, true, _>(&p, w, h, w * 3, 0, p.g.bh, &mut rgb, mode, chunk, &mut emit)
}

fn decode_band<const PRUNE: bool, const STREAM: bool, F: FnMut(usize, &[u8]) -> bool>(
    p: &LossyParsed<'_>,
    w: usize,
    h: usize,
    stride: usize,
    first: usize,
    last: usize,
    dst: &mut [u8],
    mode: Mode,
    chunk: usize,
    emit: &mut F,
) -> Result<(), Error> {
    let (step, bw) = (mode.step(), p.g.bw);
    let mut sc = Scratch::default();
    let pw = chunk * 8;
    let ps = chunk * step;
    let expanded = bw.div_ceil(chunk) * chunk * 8;
    let mut current = vec![0.0f32; 3 * 8 * pw];
    let mut packed = vec![0.0f32; 3 * ps];
    let mut previous = if mode == Mode::Blend {
        vec![0.0f32; 3 * expanded]
    } else {
        Vec::new()
    };
    let mut above_packed = if mode == Mode::Blend {
        vec![0.0f32; ps]
    } else {
        Vec::new()
    };
    let start = if mode == Mode::Blend {
        first.saturating_sub(1)
    } else {
        first
    };
    let dcp = p.dc.as_ref().unwrap();
    assert!(dcp.hs.is_multiple_of(p.g.sb));
    let dc_map = crate::v1::OutMap::new(&dcp.info, &dcp.layout);
    let dc_stride = dcp.info.row_bytes();
    let mut cursor: Option<(usize, crate::v1::UnitRows<'_>, usize)> = None;
    let (mut dc_pixels, mut dc, mut aq) = (Vec::new(), Vec::new(), Vec::new());
    let mut loaded = usize::MAX;
    for by in start..last {
        let s = by / p.g.sb;
        if loaded != s {
            let d = s * p.g.sb / dcp.hs;
            if cursor.as_ref().is_none_or(|c| c.0 != d) {
                cursor = Some((d, crate::v1::UnitRows::new(dcp, d)?, 0));
            }
            let (_, rows, next) = cursor.as_mut().unwrap();
            let first_dc_row = s * p.g.sb - d * dcp.hs;
            let nb = p.g.sb.min(p.g.bh - s * p.g.sb);
            assert!(*next <= first_dc_row);
            dc_pixels.resize(nb * dc_stride, 0);
            // A band may start inside a DC stripe. Decode its prefix into
            // the first row and overwrite it, as the production route does.
            while *next < first_dc_row + nb {
                let row = next.saturating_sub(first_dc_row);
                rows.next_row(dcp, &dc_map, &mut dc_pixels[row * dc_stride..][..dc_stride])?;
                *next += 1;
            }
            dc.clear();
            aq.clear();
            dc_values(dcp.info.sample, p.aq, false, &dc_pixels, &mut dc, &mut aq)?;
            decode_stripe_coefs(p, &p.g, s, s, &dc, true, &mut sc)?;
            loaded = s;
        }
        let r = by % p.g.sb;
        let rows = step.min(h.saturating_sub(by * step));
        let mut left = [[0.0f32; 8]; 3];
        let mut above_left = [0.0f32; 3];
        for bx0 in (0..bw).step_by(chunk) {
            let bx1 = (bx0 + chunk).min(bw);
            let nblocks = bx1 - bx0;
            let x0 = bx0 * step;
            let n = (nblocks * step).min(w - x0);
            let flat = p.bias.is_none()
                && (bx0..bx1).all(|bx| sc.eobs[(r * bw + bx) * 3..][..3].iter().all(|&e| e == 0));
            if mode != Mode::Blend && by >= first && flat {
                for bx in bx0..bx1 {
                    let b = r * bw + bx;
                    let value = core::array::from_fn(|c| {
                        let v = sc.q[(b * 3 + c) * 64] as f32 * p.dq[c][0];
                        sc.q[(b * 3 + c) * 64] = 0;
                        v
                    });
                    let rgb = color::inverse_px(p.color, p.inv_scale, value);
                    let cols = step.min(w - bx * step);
                    for y in 0..rows {
                        dst[row_offset::<STREAM>(by, first, step, y) * stride + bx * step * 3..]
                            [..cols * 3]
                            .as_chunks_mut::<3>()
                            .0
                            .fill(rgb);
                    }
                }
                continue;
            }
            let aqr = if aq.is_empty() {
                &[][..]
            } else {
                &aq[r * bw..][..bw]
            };
            if PRUNE && mode == Mode::Own && p.bias.is_none() {
                pruned::row(
                    &mut sc.q,
                    &sc.eobs,
                    r,
                    bw,
                    bx0,
                    bx1,
                    &p.dq,
                    aqr,
                    &mut current,
                    pw,
                );
            } else {
                simd::recon_block_row(
                    &mut sc.q,
                    &sc.eobs,
                    r,
                    bw,
                    bx0,
                    bx1,
                    &p.dq,
                    aqr,
                    p.bias.as_deref(),
                    &mut current,
                    pw,
                    simd::Small::NONE,
                    true,
                );
            }
            let mut constants = [[0u8; 3]; 64];
            if mode == Mode::Blend && flat && by >= first {
                for bx in 0..nblocks {
                    let value = core::array::from_fn(|c| current[c * 8 * pw + bx * 8]);
                    constants[bx] = color::inverse_px(p.color, p.inv_scale, value);
                }
            }
            if by >= first {
                for y in 0..rows {
                    let rgb = &mut dst
                        [row_offset::<STREAM>(by, first, step, y) * stride + x0 * 3..][..n * 3];
                    if mode == Mode::Blend && flat && (y != 0 || by == 0) {
                        for bx in 0..nblocks {
                            let cols = step.min(n - bx * step);
                            if cols == 0 {
                                break;
                            }
                            let target = &mut rgb[bx * step * 3..][..cols * 3];
                            target.as_chunks_mut::<3>().0.fill(constants[bx]);
                            if bx0 + bx > 0 {
                                let value = core::array::from_fn(|c| {
                                    let b = current[(c * 8 + y) * pw + bx * 8];
                                    let a = if bx == 0 {
                                        left[c][y]
                                    } else {
                                        current[(c * 8 + y) * pw + (bx - 1) * 8 + 7]
                                    };
                                    (a + b) * 0.5
                                });
                                target[..3].copy_from_slice(&color::inverse_px(
                                    p.color,
                                    p.inv_scale,
                                    value,
                                ));
                            }
                        }
                        continue;
                    }
                    for c in 0..3 {
                        let cur = &current[(c * 8 + y) * pw..][..pw];
                        let target = &mut packed[c * ps..][..ps];
                        if mode == Mode::Base {
                            target[..n].copy_from_slice(&cur[..n]);
                            continue;
                        }
                        if mode != Mode::Blend || y != 0 || by == 0 {
                            // The whole internal chunk is initialized. Packing
                            // its padded tail avoids partial-vector stores;
                            // final colour conversion still admits only n pixels.
                            super::packer::row(cur, target, chunk);
                            if mode == Mode::Blend {
                                for bx in 0..nblocks {
                                    if bx0 + bx > 0 {
                                        let a = if bx == 0 {
                                            left[c][y]
                                        } else {
                                            cur[(bx - 1) * 8 + 7]
                                        };
                                        target[bx * step] = (a + cur[bx * 8]) * 0.5;
                                    }
                                }
                            }
                            continue;
                        }
                        super::packer::row(cur, target, chunk);
                        super::packer::row(
                            &previous[c * expanded + bx0 * 8..][..pw],
                            &mut above_packed,
                            chunk,
                        );
                        super::packer::vertical_mean(&above_packed, target);
                        for bx in 0..nblocks {
                            if bx0 + bx > 0 {
                                let tr = previous[c * expanded + (bx0 + bx) * 8];
                                let br = cur[bx * 8];
                                let tl = if bx == 0 {
                                    above_left[c]
                                } else {
                                    previous[c * expanded + (bx0 + bx - 1) * 8 + 7]
                                };
                                let bl = if bx == 0 {
                                    left[c][0]
                                } else {
                                    cur[(bx - 1) * 8 + 7]
                                };
                                // An average of two horizontal averages
                                // would round differently at this corner.
                                target[bx * step] = ((tl + tr) + bl + br) * 0.25;
                            }
                        }
                    }
                    simd::color_row(
                        p.color,
                        &packed[..ps],
                        &packed[ps..2 * ps],
                        &packed[2 * ps..],
                        p.inv_scale,
                        rgb,
                        n,
                    );
                }
            }
            if mode == Mode::Blend {
                for c in 0..3 {
                    above_left[c] = previous[c * expanded + bx1 * 8 - 1];
                    previous[c * expanded + bx0 * 8..][..nblocks * 8]
                        .copy_from_slice(&current[(c * 8 + 7) * pw..][..nblocks * 8]);
                    for y in 0..8 {
                        left[c][y] = current[(c * 8 + y) * pw + nblocks * 8 - 1];
                    }
                }
            }
        }
        // A leading halo reads only the last row of the preceding unit.
        // Clear its unconsumed rows before reusing coefficient capacity.
        if by < first {
            sc.q.fill(0);
        }
        if STREAM && by >= first {
            for y in 0..rows {
                if !emit(by * step + y, &dst[y * stride..][..w * 3]) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}
