//! Synthetic precision audit. Uses production color/DCT kernels; no entropy,
//! quality metric, file-size, or throughput claims. Diagnostics separate color
//! conversion, unquantized DCT arithmetic, and q116 coefficient quantization.
use super::*;
use alloc::format;
// The test harness has std even when the codec is built with alloc/core only.
extern crate std;
use self::std::eprintln;

#[derive(Default, Debug)]
struct ErrorCounts {
    pixels: u64,
    changed_pixels: u64,
    changed_components: u64,
    absolute_error: u64,
    maximum: u16,
    worst: Option<([u16; 3], [u16; 3])>,
}
impl ErrorCounts {
    fn add(&mut self, source: [u16; 3], decoded: [u16; 3]) {
        self.pixels += 1;
        self.changed_pixels += u64::from(source != decoded);
        for c in 0..3 {
            let error = source[c].abs_diff(decoded[c]);
            self.changed_components += u64::from(error != 0);
            self.absolute_error += u64::from(error);
            if error > self.maximum {
                self.maximum = error;
                self.worst = Some((source, decoded));
            }
        }
    }
    fn report(&self, label: &str) {
        eprintln!("PRECISION {label}: {self:?}");
    }
}

fn forward(planes: &mut [Vec<f32>; 3], xyb: bool, scale: [f32; 3]) {
    let [a, b, c] = planes;
    enc_simd::color_rows(
        xyb,
        scale,
        &color::OPSIN_F32,
        color::OPSIN_BIAS,
        color::CBRT_BIAS,
        a,
        b,
        c,
    );
}

fn lift(rgb: [i64; 3]) -> [i64; 3] {
    let co = rgb[0] - rgb[2];
    let t = rgb[2] + (co >> 1);
    let cg = rgb[1] - t;
    [t + (cg >> 1), co, cg]
}
fn unlift(ycc: [i64; 3]) -> [i64; 3] {
    let t = ycc[0] - (ycc[2] >> 1);
    let g = ycc[2] + t;
    let b = t - (ycc[1] >> 1);
    [b + ycc[1], g, b]
}

#[test]
fn integer_lifting_keeps_headroom_and_reverses_boundaries() {
    // Every legal scalar value in seven axis/neutral/complement arrangements,
    // at every depth1..16. Wide arithmetic independently covers32-bit corners.
    let mut samples = 0u64;
    for bits in 1..=16 {
        let max = (1i64 << bits) - 1;
        for v in 0..=max {
            for rgb in [
                [v, v, v],
                [v, 0, 0],
                [0, v, 0],
                [0, 0, v],
                [v, max, 0],
                [max, 0, v],
                [max - v, v, max],
            ] {
                let ycc = lift(rgb);
                assert!((0..=max).contains(&ycc[0]));
                assert!((-max..=max).contains(&ycc[1]));
                assert!((-max..=max).contains(&ycc[2]));
                assert_eq!(unlift(ycc), rgb);
                samples += 1;
            }
        }
    }
    for r in [0, 1, (1i64 << 31) - 1, 1i64 << 31, (1i64 << 32) - 1] {
        for g in [0, 1, (1i64 << 32) - 1] {
            for b in [0, 1, (1i64 << 32) - 1] {
                assert_eq!(unlift(lift([r, g, b])), [r, g, b]);
            }
        }
    }
    let red = [255, 0, 0];
    let coded = lift(red);
    assert_eq!(coded, [63, 255, -127]);
    let truncated = [coded[0], coded[1] as i8 as i64, coded[2] as i8 as i64];
    assert_ne!(unlift(truncated), red);
    eprintln!(
        "PRECISION integer lifting: {samples} axis/neutral/complement cases,32-bit corners; signed chroma needs bits+1"
    );
}

#[test]
fn xyb_rgb8_exhaustive_without_dct() {
    let scale = LossyColor::Xyb.default_scales();
    let inv = scale.map(|v| 1.0 / v);
    let mut counts = ErrorCounts::default();
    for r in 0..256 {
        for g in 0..256 {
            let mut planes = [
                vec![color::SRGB8_TO_LINEAR[r] as f32; 256],
                vec![color::SRGB8_TO_LINEAR[g] as f32; 256],
                (0..256).map(|b| color::SRGB8_TO_LINEAR[b] as f32).collect(),
            ];
            forward(&mut planes, true, scale);
            for b in 0..256 {
                let decoded =
                    color::inverse_px(LossyColor::Xyb, inv, core::array::from_fn(|c| planes[c][b]));
                counts.add([r as u16, g as u16, b as u16], decoded.map(u16::from));
            }
        }
    }
    counts.report("RGB8 XYB color-only exhaustive");
    assert_eq!(counts.pixels, 1 << 24);
    // Existing sparse-grid coverage promises <=1 code. Extend that actual
    // reconstruction bound to every RGB8 tuple, rather than assuming exactness.
    assert!(counts.maximum <= 1, "{counts:?}");
}

fn corpus16() -> Vec<[u16; 3]> {
    let mut cases = Vec::with_capacity(600_000);
    for v in 0..=u16::MAX {
        cases.extend([
            [v, v, v],
            [v, 0, 0],
            [0, v, 0],
            [0, 0, v],
            [v, u16::MAX, 0],
            [u16::MAX, 0, v],
            [u16::MAX - v, v, u16::MAX],
        ]);
    }
    let edge = [
        0, 1, 2, 3, 15, 16, 255, 256, 511, 512, 1023, 1024, 4095, 4096, 16383, 16384, 32767, 32768,
        65533, 65534, 65535,
    ];
    for r in edge {
        for g in edge {
            for b in edge {
                cases.push([r, g, b]);
            }
        }
    }
    let mut seed = 0x91a2_c7d3u32;
    for _ in 0..65_536 {
        cases.push(core::array::from_fn(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u16
        }));
    }
    cases
}

#[test]
fn u16_color_and_transfer_error_are_measured_separately() {
    let cases = corpus16();
    let mut ycocg = ErrorCounts::default();
    for source in cases.chunks(256) {
        let n = source.len().next_multiple_of(8);
        let mut planes = core::array::from_fn(|c| {
            (0..n)
                .map(|i| source[i.min(source.len() - 1)][c] as f32)
                .collect()
        });
        let scale = [1.0 / 257.0; 3];
        forward(&mut planes, false, scale);
        for (i, &rgb) in source.iter().enumerate() {
            // Integer lifting can feed the current wire only after restoring
            // the half/quarter parity terms expected by its float inverse.
            let lifted = lift(rgb.map(i64::from));
            let parity_co = (lifted[1] & 1) as f32;
            let compatible = [
                lifted[0] as f32 + (lifted[2] & 1) as f32 * 0.5 + parity_co * 0.25,
                lifted[1] as f32,
                lifted[2] as f32 - parity_co * 0.5,
            ];
            assert_eq!(
                compatible.map(|v| (v * scale[0]).to_bits()),
                core::array::from_fn(|c| planes[c][i].to_bits())
            );
            ycocg.add(
                rgb,
                color::inverse_ycocg16(
                    scale.map(|v| 1.0 / v),
                    core::array::from_fn(|c| planes[c][i]),
                ),
            );
        }
    }
    ycocg.report("U16 YCoCg color-only sampled");
    assert_eq!(ycocg.maximum, 0, "{ycocg:?}");
    for transfer in [
        transfer::Transfer::Srgb,
        transfer::Transfer::Pq,
        transfer::Transfer::Hlg,
    ] {
        let lut: Vec<f32> = (0..65536)
            .map(|c| transfer.decode(c as f64 / 65535.0) as f32)
            .collect();
        let table = transfer.table();
        let maximum = transfer.linear_max_f32();
        let scale = LossyColor::Xyb.default_scales();
        let inv = scale.map(|v| 1.0 / v);
        let mut color_error = ErrorCounts::default();
        let mut transfer_error = ErrorCounts::default();
        for source in cases.chunks(256) {
            let n = source.len().next_multiple_of(8);
            let mut planes = core::array::from_fn(|c| {
                (0..n)
                    .map(|i| lut[source[i.min(source.len() - 1)][c] as usize])
                    .collect()
            });
            forward(&mut planes, true, scale);
            for (i, &rgb) in source.iter().enumerate() {
                transfer_error.add(
                    rgb,
                    rgb.map(|v| transfer::encode16(&table, maximum, lut[v as usize])),
                );
                let linear = color::inverse_xyb_linear(inv, core::array::from_fn(|c| planes[c][i]));
                assert!(linear.iter().all(|v| v.is_finite()));
                color_error.add(rgb, linear.map(|v| transfer::encode16(&table, maximum, v)));
            }
        }
        transfer_error.report(&format!("U16 {transfer:?} transfer-only sampled"));
        color_error.report(&format!("U16 {transfer:?} XYB+transfer sampled"));
        assert_eq!(color_error.pixels, cases.len() as u64);
        assert!(
            transfer_error.maximum <= 2,
            "{transfer:?} {transfer_error:?}"
        );
    }
}

#[test]
fn separate_color_dct_arithmetic_and_quantization() {
    let cases = corpus16();
    let cm = dct::dct_matrix();
    let unquant = dct::dequant_factors(&[1.0; 64]);
    for transfer in [
        transfer::Transfer::Srgb,
        transfer::Transfer::Pq,
        transfer::Transfer::Hlg,
    ] {
        let lut: Vec<f32> = (0..65536)
            .map(|c| transfer.decode(c as f64 / 65535.0) as f32)
            .collect();
        let table = transfer.table();
        for color in [LossyColor::YCoCg, LossyColor::Xyb] {
            let xyb = color == LossyColor::Xyb;
            let scale = if xyb {
                color.default_scales()
            } else {
                [1.0 / 257.0; 3]
            };
            let inverse = scale.map(|v| 1.0 / v);
            let steps = QParams::from_quality(116.0, color, if xyb { 2 } else { 0 }).steps();
            let mut counters: [ErrorCounts; 3] = core::array::from_fn(|_| ErrorCounts::default());
            // Fixed evenly spaced64-pixel blocks include axes, ramps, edge
            // tuples and mixed colors; this is synthetic coverage, not RD.
            for block in 0..1024 {
                let start = block * (cases.len() - 64) / 1023;
                let source = &cases[start..start + 64];
                let mut planes: [Vec<f32>; 3] = core::array::from_fn(|c| {
                    source
                        .iter()
                        .map(|p| if xyb { lut[p[c] as usize] } else { p[c] as f32 })
                        .collect()
                });
                forward(&mut planes, xyb, scale);
                let mut dct_planes = planes.clone();
                let mut quant_planes = planes.clone();
                for c in 0..3 {
                    let mut coeff = [0.0; 64];
                    enc_simd::fdct_row(&planes[c], 8, 1, &cm, &mut coeff);
                    let mut without: [f32; 64] = core::array::from_fn(|i| coeff[i] * unquant[i]);
                    dct::idct8x8(&mut without);
                    dct_planes[c].copy_from_slice(&without);
                    let mut q = [0; 192];
                    let mut nz = [0; 3];
                    enc_simd::quant_row(
                        &coeff,
                        1,
                        c,
                        &steps[c].map(|v| 1.0 / v),
                        &[],
                        0.4,
                        &mut q,
                        &mut nz,
                    );
                    let factors = dct::dequant_factors(&steps[c]);
                    let mut with: [f32; 64] =
                        core::array::from_fn(|i| q[c * 64 + i] as f32 * factors[i]);
                    dct::idct8x8(&mut with);
                    quant_planes[c].copy_from_slice(&with);
                }
                for (stage, p) in [&planes, &dct_planes, &quant_planes]
                    .into_iter()
                    .enumerate()
                {
                    for i in 0..64 {
                        let coded = core::array::from_fn(|c| p[c][i]);
                        let output = if xyb {
                            color::inverse_xyb_linear(inverse, coded)
                                .map(|v| transfer::encode16(&table, transfer.linear_max_f32(), v))
                        } else {
                            color::inverse_ycocg16(inverse, coded)
                        };
                        counters[stage].add(source[i], output);
                    }
                }
            }
            for (label, counts) in ["color-only", "unquantized-DCT", "q116"]
                .into_iter()
                .zip(counters.iter())
            {
                counts.report(&format!("U16 {transfer:?} {color:?} {label} blocks"));
                assert_eq!(counts.pixels, 65536);
            }
        }
    }
}
