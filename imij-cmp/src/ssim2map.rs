//! SSIMULACRA2 as a spatial map. fast-ssim2 returns only the score; this
//! reruns its pipeline (`unstable-internals`, pinned in Cargo.toml) and
//! attributes the score's linear sum S = sum_k WEIGHT[k] * f_k to pixels.
//! Each of the 108 terms is a norm over one scale and XYB channel of one
//! per-pixel error (1 - SSIM, edge artifact, detail lost): a pixel's share
//! of an L1 term is w * d / N, of an L4 term w * d^4 / (N * L4^3), so the
//! shares of every term sum to w * f_k. Scale-s shares spread evenly over
//! the full-resolution pixels they cover. The map sums to S; the score is
//! 100 - 10 * poly(0.956 * S)^0.628, falling as S grows.
use fast_ssim2::pipeline::{
    self as p, EncodedData, EncodedSrgb, gauss::create_recursive_gaussian, score::ScaleAggregates,
};

/// fast-ssim2's (and libjxl's) final-score weights, copied from
/// fast-ssim2 `src/weights.rs` at the pinned rev (crate-private there);
/// `tests::score_and_map_match_fast_ssim2` checks them against its score.
/// Index `((c * scales + scale) * 2 + n) * 3 + m` (`scales`: how many
/// ran, 6 from 256 px up): c in X/Y/B, n in L1/L4, m in 1 - SSIM /
/// artifact / detail lost.
#[allow(clippy::excessive_precision, clippy::inconsistent_digit_grouping)]
const WEIGHT: [f64; 108] = [
    0.0,
    0.000_737_660_670_740_658_6,
    0.0,
    0.0,
    0.000_779_348_168_286_730_9,
    0.0,
    0.0,
    0.000_437_115_573_010_737_9,
    0.0,
    1.104_172_642_665_734_6,
    0.000_662_848_341_292_71,
    0.000_152_316_327_837_187_52,
    0.0,
    0.001_640_643_745_659_975_4,
    0.0,
    1.842_245_552_053_929_8,
    11.441_172_603_757_666,
    0.0,
    0.000_798_910_943_601_516_3,
    0.000_176_816_438_078_653,
    0.0,
    1.878_759_497_954_638_7,
    10.949_069_906_051_42,
    0.0,
    0.000_728_934_699_150_807_2,
    0.967_793_708_062_683_3,
    0.0,
    0.000_140_034_242_854_358_84,
    0.998_176_697_785_496_7,
    0.000_319_497_559_344_350_53,
    0.000_455_099_211_379_206_3,
    0.0,
    0.0,
    0.001_364_876_616_324_339_8,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    7.466_890_328_078_848,
    0.0,
    17.445_833_984_131_262,
    0.000_623_560_163_404_146_6,
    0.0,
    0.0,
    6.683_678_146_179_332,
    0.000_377_244_079_796_112_96,
    1.027_889_937_768_264,
    225.205_153_008_492_74,
    0.0,
    0.0,
    19.213_238_186_143_016,
    0.001_140_152_458_661_836_1,
    0.001_237_755_635_509_985,
    176.393_175_984_506_94,
    0.0,
    0.0,
    24.433_009_998_704_76,
    0.285_208_026_121_177_57,
    0.000_448_543_692_383_340_8,
    0.0,
    0.0,
    0.0,
    34.779_063_444_837_72,
    44.835_625_328_877_896,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.000_868_055_657_329_169_8,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.000_531_319_187_435_874_7,
    0.0,
    0.000_165_338_141_613_791_12,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    0.000_417_917_180_325_133_6,
    0.001_729_082_823_472_283_3,
    0.0,
    0.002_082_700_584_663_643_7,
    0.0,
    0.0,
    8.826_982_764_996_862,
    23.192_433_439_989_26,
    0.0,
    95.108_049_881_108_6,
    0.986_397_803_440_068_2,
    0.983_438_279_246_535_3,
    0.001_228_640_504_827_849_3,
    171.266_725_589_730_7,
    0.980_785_887_243_537_9,
    0.0,
    0.0,
    0.0,
    0.000_513_006_458_899_067_9,
    0.0,
    0.000_108_540_578_584_115_37,
];

/// fast-ssim2 `maps::K_C2` and `tothe4th` (crate-private there).
const K_C2: f32 = 0.0009;
fn tothe4th(mut x: f64) -> f64 {
    x *= x;
    x * x
}

pub(crate) struct Ssim2Map {
    /// The SSIMULACRA2 score (fast-ssim2's, bit for bit).
    pub score: f64,
    /// The score's linear sum S (the map's sum).
    #[cfg_attr(not(test), allow(dead_code))]
    pub sum: f64,
    /// Per-pixel share of S, full resolution, row-major.
    pub map: Vec<f32>,
}

fn encoded(px: &[u8], w: usize, h: usize) -> EncodedSrgb {
    EncodedSrgb {
        width: w,
        height: h,
        data: EncodedData::U8(px.to_vec()),
        alpha: None,
    }
}

/// Per-pixel errors of one channel: 1 - SSIM, edge artifact, detail lost
/// (fast-ssim2 `maps_fused`: f32 quotients, f64 differences).
fn terms(c: usize, i: usize, [m1, m2, s11, s22, s12, x1, x2]: [&[Vec<f32>; 3]; 7]) -> [f64; 3] {
    let (mu1, mu2) = (m1[c][i], m2[c][i]);
    let (mu11, mu22, mu12) = (mu1 * mu1, mu2 * mu2, mu1 * mu2);
    let dm = mu1 - mu2;
    let num_m = (-dm).mul_add(dm, 1.0f32);
    let num_s = 2.0f32 * (s12[c][i] - mu12) + K_C2;
    let denom_s = (s11[c][i] - mu11) + (s22[c][i] - mu22) + K_C2;
    let q = num_m * num_s / denom_s;
    let d = (1.0f64 - q as f64).max(0.0);
    let num = 1.0 + (x2[c][i] - mu2).abs() as f64;
    let den = 1.0 + (x1[c][i] - mu1).abs() as f64;
    let d1 = num / den - 1.0;
    [d, d1.max(0.0), (-d1).max(0.0)]
}

pub(crate) fn ssim2_map(src: &[u8], dst: &[u8], w: usize, h: usize) -> Ssim2Map {
    assert!(w >= 8 && h >= 8 && src.len() == w * h * 3 && dst.len() == src.len());
    // The score walks WEIGHT sequentially over the scales that run (fewer
    // than 6 below 256 px), as the reference does: count them first.
    let nscales = {
        let (mut gw, mut gh, mut sw, mut sh, mut n) = (w, h, w, h, 0);
        while n < 6 && gw >= 8 && gh >= 8 {
            n += 1;
            (gw, gh, sw, sh) = (sw, sh, sw.div_ceil(2), sh.div_ceil(2));
        }
        n
    };
    let rg = create_recursive_gaussian(1.5);
    let mut lin1 = p::linearize(&encoded(src, w, h), 0.5);
    let mut lin2 = p::linearize(&encoded(dst, w, h), 0.5);
    let (mut sw, mut sh, mut gw, mut gh) = (w, h, w, h);
    let mut scales = Vec::new();
    let mut map = vec![0f32; w * h];
    let mut sum = 0f64;
    for scale in 0..6 {
        // fast-ssim2 gates on the previous scale's size (see compute_planar_stop).
        if gw < 8 || gh < 8 {
            break;
        }
        let (next1, nw, nh) = p::downsample_planes(&lin1, sw, sh);
        let (next2, _, _) = p::downsample_planes(&lin2, sw, sh);
        let n = sw * sh;
        let (mut x1, mut x2) = (lin1, lin2);
        p::planes_to_positive_xyb(&mut x1, n);
        p::planes_to_positive_xyb(&mut x2, n);
        let blur = |a: &[Vec<f32>; 3]| p::simd::blur_planes_simd(&rg, a, sw, sh);
        let prod = |a: &[Vec<f32>; 3], b: &[Vec<f32>; 3]| {
            let mut out = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
            p::gauss::multiply_planes(a, b, &mut out);
            blur(&out)
        };
        let (s11, s22, s12) = (prod(&x1, &x1), prod(&x2, &x2), prod(&x1, &x2));
        let (m1, m2) = (blur(&x1), blur(&x2));
        let planes = [&m1, &m2, &s11, &s22, &s12, &x1, &x2];
        let opp = 1.0 / n as f64;
        let (mut avg_ssim, mut avg_edgediff) = ([0f64; 6], [0f64; 12]);
        // Norms, accumulated in fast-ssim2's order: [L1, L4] per error.
        let mut norms = [[[0f64; 2]; 3]; 3];
        for c in 0..3 {
            let mut acc = [[0f64; 2]; 3];
            for i in 0..n {
                for (a, t) in acc.iter_mut().zip(terms(c, i, planes)) {
                    a[0] += t;
                    a[1] += tothe4th(t);
                }
            }
            for (nm, a) in norms[c].iter_mut().zip(acc) {
                *nm = [opp * a[0], (opp * a[1]).sqrt().sqrt()];
            }
            avg_ssim[c * 2] = norms[c][0][0];
            avg_ssim[c * 2 + 1] = norms[c][0][1];
            for m in 1..3 {
                avg_edgediff[c * 4 + (m - 1) * 2] = norms[c][m][0];
                avg_edgediff[c * 4 + (m - 1) * 2 + 1] = norms[c][m][1];
            }
        }
        // Per-pixel share at this scale, spread over the covered pixels.
        let f = 1usize << scale;
        for c in 0..3 {
            for (m, nm) in norms[c].iter().enumerate() {
                let w1 = WEIGHT[((c * nscales + scale) * 2) * 3 + m];
                let w4 = WEIGHT[((c * nscales + scale) * 2 + 1) * 3 + m];
                sum += w1 * nm[0] + w4 * nm[1];
                let k4 = if nm[1] > 0.0 {
                    w4 * opp / nm[1].powi(3)
                } else {
                    0.0
                };
                if w1 == 0.0 && k4 == 0.0 {
                    continue;
                }
                for sy in 0..sh {
                    let (y0, y1) = (sy * f, ((sy + 1) * f).min(h));
                    for sx in 0..sw {
                        let t = terms(c, sy * sw + sx, planes)[m];
                        let share = w1 * opp * t + k4 * tothe4th(t);
                        if share == 0.0 {
                            continue;
                        }
                        let (x0, x1) = (sx * f, ((sx + 1) * f).min(w));
                        let v = (share / ((y1 - y0) * (x1 - x0)) as f64) as f32;
                        for y in y0..y1 {
                            for px in &mut map[y * w + x0..y * w + x1] {
                                *px += v;
                            }
                        }
                    }
                }
            }
        }
        scales.push(ScaleAggregates {
            avg_ssim,
            avg_edgediff,
        });
        (lin1, lin2) = (next1, next2);
        (gw, gh, sw, sh) = (sw, sh, nw, nh);
    }
    Ssim2Map {
        score: p::score::score(&scales),
        sum,
        map,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_and_map_match_fast_ssim2() {
        for (w, h) in [(300usize, 260usize), (64, 48), (37, 29), (8, 8)] {
            let src: Vec<u8> = (0..w * h * 3)
                .map(|i| ((i * 37 + (i / 7) * 11) % 251) as u8)
                .collect();
            let dst: Vec<u8> = src
                .iter()
                .enumerate()
                .map(|(i, &v)| v.saturating_add((i % 13) as u8).saturating_sub(5))
                .collect();
            let m = ssim2_map(&src, &dst, w, h);
            let d = fast_ssim2::PixelDescriptor::RGB8_SRGB;
            let (a, b) = (
                fast_ssim2::PixelSlice::new(&src, w as u32, h as u32, w * 3, d).unwrap(),
                fast_ssim2::PixelSlice::new(&dst, w as u32, h as u32, w * 3, d).unwrap(),
            );
            let expected = fast_ssim2::Ssimulacra2Reference::new(&a)
                .unwrap()
                .compare(&b)
                .unwrap();
            assert_eq!(m.score, expected, "{w}x{h}");
            // The score from the attributed sum (WEIGHT copy check).
            let s = m.sum * 0.956_238_261_683_484_4;
            let s = 2.326_765_642_916_932 * s - 0.020_884_521_182_843_837 * s * s
                + 6.248_496_625_763_138e-05 * s * s * s;
            let from_sum = if s > 0.0 {
                100.0 - 10.0 * s.powf(0.627_633_646_783_138_7)
            } else {
                100.0
            };
            assert!(
                (from_sum - expected).abs() < 1e-9,
                "{w}x{h}: {from_sum} vs {expected}"
            );
            let total: f64 = m.map.iter().map(|&v| v as f64).sum();
            assert!(
                (total - m.sum).abs() <= 1e-4 * m.sum,
                "{w}x{h}: {total} vs {}",
                m.sum
            );
        }
    }
}
