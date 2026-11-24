//! Experimental RGB8/RGBA8 seam filter; independent of the codec and bitstream.
//! Code values are assumed SDR sRGB, with straight alpha when present.
//! Stride is in bytes. Origin is relative to the encoded block grid, not the
//! uncropped photograph. Encoded crops therefore normally use origin (0, 0).
//! No heap scratch or global state. Vertical seams are completed before horizontal
//! seams; the latter read the vertically filtered pixels. Within each pass the
//! four-sample neighborhoods are disjoint (seams eight pixels apart).
//! A seam needs two pixels on each side; incomplete crop/outer halos are skipped.
//! Independent images can run concurrently. Arbitrary tiles cannot: they need
//! halos and a barrier between passes to reproduce the whole-image result.
#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Strength {
    Off,
    Mild,
    Medium,
    Strong,
}

impl Strength {
    pub(crate) const GRID: [Self; 4] = [Self::Off, Self::Mild, Self::Medium, Self::Strong];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Mild => "mild",
            Self::Medium => "medium",
            Self::Strong => "strong",
        }
    }

    pub(crate) fn parse(name: &str) -> Result<Self, &'static str> {
        Self::GRID
            .into_iter()
            .find(|s| s.name() == name)
            .ok_or("unknown strength")
    }

    // Fixed before corpus scores: (residual weight / 16, RGB gate, delta cap).
    fn parameters(self) -> (i32, i32, i32) {
        match self {
            Self::Off => (0, 0, 0),
            Self::Mild => (1, 16, 2),
            Self::Medium => (2, 24, 4),
            Self::Strong => (3, 32, 6),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride_bytes: usize,
    pub(crate) channels: usize,
    pub(crate) origin: (usize, usize),
}

impl Layout {
    pub(crate) fn validate(self, len: usize) -> Result<(), &'static str> {
        if ![3, 4].contains(&self.channels) {
            return Err("requires interleaved RGB8 or straight RGBA8");
        }
        let row = self
            .width
            .checked_mul(self.channels)
            .ok_or("row overflow")?;
        if self.stride_bytes < row {
            return Err("stride smaller than pixel row");
        }
        let end = if self.width == 0 || self.height == 0 {
            0
        } else {
            (self.height - 1)
                .checked_mul(self.stride_bytes)
                .and_then(|v| v.checked_add(row))
                .ok_or("image extent overflow")?
        };
        if end > len {
            return Err("pixel buffer too short");
        }
        Ok(())
    }
}

// Four RGB samples p1,p0 | q0,q1. The gate is joint across RGB, so a strong
// chromatic edge suppresses all channels. Unequal alpha suppresses filtering.
// Subtracting side slopes from twice the seam jump gives zero on linear ramps.
fn seam<const CHANNELS: usize>(pixels: &mut [u8], offsets: [usize; 4], params: (i32, i32, i32)) {
    let (weight, gate, cap) = params;
    if CHANNELS == 4
        && offsets
            .iter()
            .any(|&i| pixels[i + 3] != pixels[offsets[0] + 3])
    {
        return;
    }
    let mut residual = [0; 3];
    for (c, r) in residual.iter_mut().enumerate() {
        let [a, b, d, e] = offsets.map(|i| i32::from(pixels[i + c]));
        if (d - b).abs() > gate || (b - a).abs() > gate / 2 || (e - d).abs() > gate / 2 {
            return;
        }
        *r = 2 * (d - b) - (b - a) - (e - d);
    }
    for (c, r) in residual.into_iter().enumerate() {
        let jump = i32::from(pixels[offsets[2] + c]) - i32::from(pixels[offsets[1] + c]);
        // Do not steepen an already smooth transition or invert the seam.
        if r.signum() != jump.signum() {
            continue;
        }
        let scaled = r * weight;
        let bound = cap.min(jump.abs() / 2);
        let delta = (scaled.signum() * ((scaled.abs() + 8) / 16)).clamp(-bound, bound);
        let p = offsets[1] + c;
        let q = offsets[2] + c;
        pixels[p] = (i32::from(pixels[p]) + delta).clamp(0, 255) as u8;
        pixels[q] = (i32::from(pixels[q]) - delta).clamp(0, 255) as u8;
    }
}

pub(crate) fn apply(
    pixels: &mut [u8],
    layout: Layout,
    strength: Strength,
) -> Result<(), &'static str> {
    layout.validate(pixels.len())?;
    if strength == Strength::Off || layout.width == 0 || layout.height == 0 {
        return Ok(());
    }
    match layout.channels {
        3 => passes::<3>(pixels, layout, strength.parameters()),
        4 => passes::<4>(pixels, layout, strength.parameters()),
        _ => unreachable!("validated channel count"),
    }
    Ok(())
}

fn passes<const CHANNELS: usize>(pixels: &mut [u8], layout: Layout, params: (i32, i32, i32)) {
    let Layout {
        width,
        height,
        stride_bytes: stride,
        origin,
        ..
    } = layout;
    // Include seams shifted by a crop origin, without overflowing origin+x.
    let first_x = (8 - origin.0 % 8) % 8;
    for y in 0..height {
        for x in (first_x..width.saturating_sub(1))
            .step_by(8)
            .filter(|&x| x >= 2)
        {
            seam::<CHANNELS>(
                pixels,
                [x - 2, x - 1, x, x + 1].map(|v| y * stride + v * CHANNELS),
                params,
            );
        }
    }
    let first_y = (8 - origin.1 % 8) % 8;
    for y in (first_y..height.saturating_sub(1))
        .step_by(8)
        .filter(|&y| y >= 2)
    {
        for x in 0..width {
            seam::<CHANNELS>(
                pixels,
                [y - 2, y - 1, y, y + 1].map(|v| v * stride + x * CHANNELS),
                params,
            );
        }
    }
}

#[cfg(test)]
// The harness=false bench compiles with cfg(test) without executing unit tests.
// Unit tests are run directly with rustc --test on this single source file.
#[allow(dead_code)]
mod tests {
    use super::*;

    fn layout(w: usize, h: usize) -> Layout {
        Layout {
            width: w,
            height: h,
            stride_bytes: w * 3,
            channels: 3,
            origin: (0, 0),
        }
    }

    #[test]
    fn constants_and_linear_ramps_are_exact() {
        for value in [0, 1, 127, 254, 255] {
            for s in Strength::GRID {
                let mut p = vec![value; 37 * 25 * 3];
                let before = p.clone();
                apply(&mut p, layout(37, 25), s).unwrap();
                assert_eq!(p, before);
            }
        }
        for s in Strength::GRID {
            let mut p: Vec<u8> = (0..25)
                .flat_map(|y| (0..37).flat_map(move |x| [x + 2 * y; 3]))
                .collect();
            let before = p.clone();
            apply(&mut p, layout(37, 25), s).unwrap();
            assert_eq!(p, before);
        }
    }

    #[test]
    fn small_block_step_smooths_only_seam_pair() {
        let mut p: Vec<u8> = (0..16)
            .flat_map(|x| [if x < 8 { 100 } else { 116 }; 3])
            .collect();
        apply(&mut p, layout(16, 1), Strength::Medium).unwrap();
        assert_eq!(
            &p[18..30],
            &[100, 100, 100, 104, 104, 104, 112, 112, 112, 116, 116, 116]
        );
    }

    #[test]
    fn real_luma_chroma_edges_and_impulses_survive() {
        for s in Strength::GRID {
            for right in [[255; 3], [100, 200, 100]] {
                let mut p: Vec<u8> = (0..17)
                    .flat_map(|x| if x < 8 { [100; 3] } else { right })
                    .collect();
                let before = p.clone();
                apply(&mut p, layout(17, 1), s).unwrap();
                assert_eq!(p, before);
            }
            for x in [6, 7, 8, 9] {
                let mut p = vec![0; 17 * 3];
                p[x * 3..x * 3 + 3].fill(255);
                let before = p.clone();
                apply(&mut p, layout(17, 1), s).unwrap();
                assert_eq!(p, before);
            }
        }
    }

    #[test]
    fn off_tiny_odd_stride_alpha_and_invalid_extents() {
        for w in 0..19 {
            for h in 0..19 {
                let l = Layout {
                    width: w,
                    height: h,
                    channels: 4,
                    stride_bytes: w * 4 + 5,
                    origin: (usize::MAX, 5),
                };
                let mut p = vec![177; l.stride_bytes * h + 7];
                for y in 0..h {
                    for x in 0..w {
                        p[y * l.stride_bytes + x * 4] = ((x * 11 + y * 7) % 256) as u8;
                    }
                }
                let before = p.clone();
                apply(&mut p, l, Strength::Off).unwrap();
                assert_eq!(p, before);
                apply(&mut p, l, Strength::Strong).unwrap();
                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(
                            p[y * l.stride_bytes + x * 4 + 3],
                            before[y * l.stride_bytes + x * 4 + 3]
                        );
                    }
                    assert_eq!(
                        &p[y * l.stride_bytes + w * 4..(y + 1) * l.stride_bytes],
                        &before[y * l.stride_bytes + w * 4..(y + 1) * l.stride_bytes]
                    );
                }
                assert_eq!(&p[l.stride_bytes * h..], &before[l.stride_bytes * h..]);
            }
        }
        assert!(apply(&mut [0; 5], layout(2, 1), Strength::Off).is_err());
        let invalid = Layout {
            width: usize::MAX,
            height: 2,
            stride_bytes: usize::MAX,
            channels: 3,
            origin: (0, 0),
        };
        assert!(apply(&mut [], invalid, Strength::Off).is_err());
    }

    #[test]
    fn crop_origin_and_horizontal_step() {
        let mut p: Vec<u8> = (0..13)
            .flat_map(|y| [if y < 5 { 0 } else { 16 }; 3])
            .collect();
        let l = Layout {
            origin: (0, 3),
            ..layout(1, 13)
        };
        apply(&mut p, l, Strength::Medium).unwrap();
        assert_eq!(&p[9..21], &[0, 0, 0, 4, 4, 4, 12, 12, 12, 16, 16, 16]);
    }

    #[test]
    fn different_alpha_blocks_rgb_changes() {
        let l = Layout {
            channels: 4,
            stride_bytes: 64,
            ..layout(16, 1)
        };
        let mut p: Vec<u8> = (0..16)
            .flat_map(|x| [if x < 8 { 100 } else { 116 }, 100, 100, x])
            .collect();
        let before = p.clone();
        apply(&mut p, l, Strength::Strong).unwrap();
        assert_eq!(p, before);
    }

    #[test]
    fn concurrent_images_match_and_changes_are_bounded() {
        let mut seed = 42u32;
        let p: Vec<u8> = (0..33 * 27 * 3)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        std::thread::scope(|scope| {
            let handles: Vec<_> = Strength::GRID
                .into_iter()
                .map(|s| {
                    let p = &p;
                    scope.spawn(move || {
                        let mut out = p.clone();
                        apply(&mut out, layout(33, 27), s).unwrap();
                        (s, out)
                    })
                })
                .collect();
            for handle in handles {
                let (s, out) = handle.join().unwrap();
                let mut sequential = p.clone();
                apply(&mut sequential, layout(33, 27), s).unwrap();
                assert_eq!(out, sequential);
                assert!(out.iter().zip(&p).all(|(&a, &b)| a.abs_diff(b) <= 12));
            }
        });
    }

    #[test]
    fn seam_extremes_conserve_pair_and_never_invert_or_steepen() {
        let values = [0, 1, 15, 16, 127, 239, 240, 254, 255];
        for s in Strength::GRID {
            assert_eq!(Strength::parse(s.name()), Ok(s));
            for a in values {
                for b in values {
                    for q in values {
                        for e in values {
                            let mut p = [a, a, a, b, b, b, q, q, q, e, e, e];
                            seam::<3>(&mut p, [0, 3, 6, 9], s.parameters());
                            assert_eq!(&p[..3], &[a; 3]);
                            assert_eq!(&p[9..], &[e; 3]);
                            let before = i32::from(q) - i32::from(b);
                            let after = i32::from(p[6]) - i32::from(p[3]);
                            assert!(after.abs() <= before.abs());
                            assert!(after == 0 || after.signum() == before.signum());
                            assert_eq!(
                                u16::from(p[3]) + u16::from(p[6]),
                                u16::from(b) + u16::from(q)
                            );
                            assert!(p[3].abs_diff(b) <= s.parameters().2 as u8);
                        }
                    }
                }
            }
        }
        assert!(Strength::parse("invalid").is_err());
    }

    #[test]
    fn row_order_matches_original_column_traversal_rgb_and_rgba() {
        for channels in [3, 4] {
            for ox in 0..8 {
                for oy in 0..8 {
                    let l = Layout {
                        width: 33,
                        height: 27,
                        stride_bytes: 33 * channels + 5,
                        channels,
                        origin: (ox, oy),
                    };
                    let mut source = vec![99; l.stride_bytes * l.height];
                    for y in 0..l.height {
                        for x in 0..l.width {
                            for c in 0..3 {
                                source[y * l.stride_bytes + x * channels + c] =
                                    (30 + 12 * (x / 8) + 9 * (y / 8) + c + (x + y) % 3) as u8;
                            }
                        }
                    }
                    for s in Strength::GRID.into_iter().skip(1) {
                        let mut reference = source.clone();
                        let mut call = |offsets| {
                            if channels == 3 {
                                seam::<3>(&mut reference, offsets, s.parameters());
                            } else {
                                seam::<4>(&mut reference, offsets, s.parameters());
                            }
                        };
                        for x in ((8 - ox) % 8..l.width - 1).step_by(8).filter(|&x| x >= 2) {
                            for y in 0..l.height {
                                call(
                                    [x - 2, x - 1, x, x + 1]
                                        .map(|v| y * l.stride_bytes + v * channels),
                                );
                            }
                        }
                        for y in ((8 - oy) % 8..l.height - 1).step_by(8).filter(|&y| y >= 2) {
                            for x in 0..l.width {
                                call(
                                    [y - 2, y - 1, y, y + 1]
                                        .map(|v| v * l.stride_bytes + x * channels),
                                );
                            }
                        }
                        let mut row_order = source.clone();
                        apply(&mut row_order, l, s).unwrap();
                        assert_eq!(
                            row_order, reference,
                            "channels={channels} origin=({ox},{oy}) strength={s:?}"
                        );
                    }
                }
            }
        }
    }
}
