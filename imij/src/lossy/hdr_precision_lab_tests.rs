//! Bounded HDR arithmetic lab. Production forward/inverse stay unchanged.
//! Independent std f64 transfer, root, and matrix inversion form the oracle.
extern crate std;
use super::{LossyColor, color, enc_simd, transfer};
use alloc::{vec, vec::Vec};
use std::eprintln;

const EDGE: [u16; 21] = [
    0, 1, 2, 3, 15, 16, 255, 256, 511, 512, 1023, 1024, 4095, 4096, 16383, 16384, 32767, 32768,
    65533, 65534, 65535,
];
const LABELS: [&str; 11] = [
    "production",
    "exact_root_f32",
    "encoder_f64",
    "inverse_matrix_f64",
    "inverse_f64_norm",
    "inverse_f64_matched",
    "encoder_inverse_f64_cast",
    "unscaled_f32",
    "all_f64",
    "all_f64_norm",
    "all_f64_rounded_matrix",
];

fn decode(t: transfer::Transfer, c: f64) -> f64 {
    match t {
        transfer::Transfer::Srgb => {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        transfer::Transfer::Pq => {
            let p = c.powf(1.0 / 78.84375);
            ((p - 0.8359375).max(0.0) / (18.8515625 - 18.6875 * p)).powf(1.0 / 0.1593017578125)
                * 49.25
        }
        transfer::Transfer::Hlg => {
            (if c <= 0.5 {
                c * c / 3.0
            } else {
                (((c - 0.55991073) / 0.17883277).exp() + 0.28466892) / 12.0
            }) * 3.78125
        }
    }
}
fn encode(t: transfer::Transfer, l: f64) -> f64 {
    let l = l.clamp(0.0, t.linear_max_f32() as f64);
    match t {
        transfer::Transfer::Srgb => {
            if l <= 0.0031308 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            }
        }
        transfer::Transfer::Pq => {
            let p = (l / 49.25).powf(0.1593017578125);
            ((0.8359375 + 18.8515625 * p) / (1.0 + 18.6875 * p)).powf(78.84375)
        }
        transfer::Transfer::Hlg => {
            let e = l / 3.78125;
            if e <= 1.0 / 12.0 {
                (3.0 * e).sqrt()
            } else {
                0.17883277 * (12.0 * e - 0.28466892).ln() + 0.55991073
            }
        }
    }
    .clamp(0.0, 1.0)
}
fn code(t: transfer::Transfer, l: f64, max: u16) -> u16 {
    (encode(t, l) * f64::from(max)).round_ties_even() as u16
}

fn invert(a: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m: [[f64; 6]; 3] = core::array::from_fn(|r| {
        core::array::from_fn(|c| {
            if c < 3 {
                a[r][c]
            } else if c - 3 == r {
                1.0
            } else {
                0.0
            }
        })
    });
    for p in 0..3 {
        let d = m[p][p];
        assert!(d.abs() > 1e-4);
        for c in 0..6 {
            m[p][c] /= d;
        }
        for r in 0..3 {
            if r != p {
                let k = m[r][p];
                for c in 0..6 {
                    m[r][c] -= k * m[p][c];
                }
            }
        }
    }
    core::array::from_fn(|r| core::array::from_fn(|c| m[r][c + 3]))
}
fn matrix(a: [[f64; 3]; 3], l: [f64; 3]) -> [f64; 3] {
    a.map(|r| r[0] * l[0] + r[1] * l[1] + r[2] * l[2])
}
fn forward64(l: [f64; 3], scale: [f32; 3]) -> [f64; 3] {
    let roots = matrix(color::OPSIN_F32.map(|r| r.map(f64::from)), l)
        .map(|m| (m + f64::from(color::OPSIN_BIAS)).max(0.0).cbrt() - f64::from(color::CBRT_BIAS));
    let y = (roots[0] + roots[1]) * 0.5;
    let v = [y, (roots[0] - roots[1]) * 0.5, roots[2] - y];
    core::array::from_fn(|c| v[c] * f64::from(scale[c]))
}
fn exact_root32(l: [f32; 3], scale: [f32; 3]) -> [f32; 3] {
    let roots = color::OPSIN_F32.map(|r| {
        let m = r[0] * l[0] + r[1] * l[1] + r[2] * l[2] + color::OPSIN_BIAS;
        (f64::from(m.max(0.0)).cbrt() as f32) - color::CBRT_BIAS
    });
    let y = (roots[0] + roots[1]) * 0.5;
    [
        y * scale[0],
        (roots[0] - roots[1]) * 0.5 * scale[1],
        (roots[2] - y) * scale[2],
    ]
}
fn inverse64(v: [f64; 3], scale: [f32; 3], k: [[f64; 3]; 3], matched: bool) -> [f64; 3] {
    let u: [f64; 3] = core::array::from_fn(|c| {
        if matched {
            v[c] / f64::from(scale[c])
        } else {
            v[c] * f64::from(1.0 / scale[c])
        }
    });
    let roots = [u[0] + u[1], u[0] - u[1], u[2] + u[0]];
    let lms = roots.map(|x| {
        let x = (x + f64::from(color::CBRT_BIAS)).clamp(-64.0, 64.0);
        x * x * x - f64::from(color::OPSIN_BIAS)
    });
    matrix(k, lms)
}
fn inverse_matrix64(v: [f32; 3], inv: [f32; 3]) -> [f64; 3] {
    let y = v[0] * inv[0];
    let x = v[1] * inv[1];
    let b = v[2] * inv[2] + y;
    let lms = [y + x, y - x, b].map(|x| {
        let x = (x + color::CBRT_BIAS).clamp(-64.0, 64.0);
        f64::from(x * x * x - color::OPSIN_BIAS)
    });
    matrix(color::INV_OPSIN.map(|r| r.map(f64::from)), lms)
}
fn production_forward(planes: &mut [Vec<f32>; 3]) {
    let [a, b, c] = planes;
    enc_simd::color_rows(
        true,
        LossyColor::Xyb.default_scales(),
        &color::OPSIN_F32,
        color::OPSIN_BIAS,
        color::CBRT_BIAS,
        a,
        b,
        c,
    );
}

#[derive(Default)]
struct Stats {
    pixels: u64,
    changed: u64,
    components: u64,
    squared: u64,
    max: u16,
    table_delta: u16,
    linear_max: f64,
    negative: u64,
    worst: ([u16; 3], [u16; 3]),
}
impl Stats {
    fn add(
        &mut self,
        rgb: [u16; 3],
        out: [u16; 3],
        analytic: [u16; 3],
        linear: [f64; 3],
        input: [f64; 3],
    ) {
        self.pixels += 1;
        self.changed += u64::from(rgb != out);
        for c in 0..3 {
            let e = rgb[c].abs_diff(out[c]);
            self.components += u64::from(e != 0);
            self.squared += u64::from(e) * u64::from(e);
            self.table_delta = self.table_delta.max(out[c].abs_diff(analytic[c]));
            self.linear_max = self.linear_max.max((linear[c] - input[c]).abs());
            self.negative += u64::from(linear[c] < 0.0);
            if e > self.max {
                self.max = e;
                self.worst = (rgb, out);
            }
        }
    }
    fn report(&self, t: transfer::Transfer, label: &str, grid: &str) {
        eprintln!(
            "HDRLAB\t{t:?}\t{grid}\t{label}\t{}\t{}\t{}\t{}\t{:.9}\t{}\t{:.12e}\t{}\t{:?}\t{:?}",
            self.pixels,
            self.changed,
            self.components,
            self.max,
            (self.squared as f64 / (self.pixels * 3) as f64).sqrt(),
            self.table_delta,
            self.linear_max,
            self.negative,
            self.worst.0,
            self.worst.1
        );
    }
}
fn seven(v: u16, max: u16) -> [[u16; 3]; 7] {
    [
        [v, v, v],
        [v, 0, 0],
        [0, v, 0],
        [0, 0, v],
        [v, max, 0],
        [max, 0, v],
        [max - v, v, max],
    ]
}
fn cases(max: u16, random: bool) -> Vec<[u16; 3]> {
    let mut result = Vec::new();
    for v in 0..=max {
        result.extend(seven(v, max));
    }
    let mut edge: Vec<_> = EDGE.into_iter().filter(|&v| v <= max).collect();
    if !edge.contains(&max) {
        edge.push(max);
    }
    for &r in &edge {
        for &g in &edge {
            for &b in &edge {
                result.push([r, g, b]);
            }
        }
    }
    if random {
        let mut seed = 0x91a2_c7d3u32;
        for _ in 0..65536 {
            result.push(core::array::from_fn(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u16
            }));
        }
        result.extend([[1866, 65535, 0], [0, 64483, 0], [0, 65097, 0]]);
    }
    result
}
fn evaluate(t: transfer::Transfer, rgb: &[[u16; 3]], max: u16, grid: &str) -> [Stats; 11] {
    let scale = LossyColor::Xyb.default_scales();
    let inv = scale.map(|s| 1.0 / s);
    let matched = invert(color::OPSIN_F32.map(|r| r.map(f64::from)));
    let norm = color::INV_OPSIN.map(|r| r.map(f64::from));
    let table = t.table();
    let lut: Vec<f32> = (0..=max)
        .map(|v| t.decode(f64::from(v) / f64::from(max)) as f32)
        .collect();
    let oracle: Vec<f64> = (0..=max)
        .map(|v| decode(t, f64::from(v) / f64::from(max)))
        .collect();
    for (&a, &b) in lut.iter().zip(&oracle) {
        assert!((f64::from(a) - b).abs() <= 4e-6 * b.abs().max(1e-12));
    }
    let mut stats: [Stats; 11] = core::array::from_fn(|_| Stats::default());
    for chunk in rgb.chunks(256) {
        let n = chunk.len().next_multiple_of(8);
        let mut planes = core::array::from_fn(|c| {
            (0..n)
                .map(|i| lut[chunk[i.min(chunk.len() - 1)][c] as usize])
                .collect()
        });
        production_forward(&mut planes);
        for (i, &source) in chunk.iter().enumerate() {
            let l = source.map(|v| lut[v as usize]);
            let input = source.map(|v| oracle[v as usize]);
            let p: [f32; 3] = core::array::from_fn(|c| planes[c][i]);
            // Tie the diagnostic decomposition to this dispatched implementation.
            assert_eq!(
                p.map(f32::to_bits),
                color::forward_xyb_linear(scale, l).map(f32::to_bits)
            );
            let p64 = forward64(l.map(f64::from), scale);
            let cast = p64.map(|v| v as f32);
            let linear = [
                color::inverse_xyb_linear(inv, p).map(f64::from),
                color::inverse_xyb_linear(inv, exact_root32(l, scale)).map(f64::from),
                color::inverse_xyb_linear(inv, cast).map(f64::from),
                inverse_matrix64(p, inv),
                inverse64(p.map(f64::from), scale, norm, false),
                inverse64(p.map(f64::from), scale, matched, true),
                inverse64(cast.map(f64::from), scale, matched, true),
                color::inverse_xyb_linear([1.0; 3], color::forward_xyb_linear([1.0; 3], l))
                    .map(f64::from),
                inverse64(forward64(input, scale), scale, matched, true),
                inverse64(forward64(input, scale), scale, norm, false),
                inverse64(forward64(input, scale), scale, norm, true),
            ];
            for a in 0..11 {
                let analytic = linear[a].map(|v| code(t, v, max));
                // All-f64 is an oracle, not a proposed production-table decoder.
                let out = if a == 8 {
                    analytic
                } else {
                    linear[a].map(|v| {
                        transfer::encode_code(&table, t.linear_max_f32(), v as f32, f32::from(max))
                    })
                };
                stats[a].add(source, out, analytic, linear[a], input);
            }
        }
    }
    for (s, label) in stats.iter().zip(LABELS) {
        s.report(t, label, grid);
    }
    assert_eq!(stats[8].max, 0, "matched f64 oracle");
    stats
}

#[test]
fn hdr_retained_failures_and_transfer_are_independent() {
    let examples = [
        (transfer::Transfer::Pq, [0, 64483, 0], [3131, 64483, 0]),
        (transfer::Transfer::Hlg, [0, 65097, 0], [261, 65097, 0]),
    ];
    for (t, rgb, expected) in examples {
        let scale = LossyColor::Xyb.default_scales();
        let l = rgb.map(|v| t.decode(f64::from(v) / 65535.0) as f32);
        let mut planes = l.map(|v| vec![v; 8]);
        production_forward(&mut planes);
        let p = core::array::from_fn(|c| planes[c][0]);
        let table = t.table();
        let out = color::inverse_xyb_linear(scale.map(|v| 1.0 / v), p)
            .map(|v| transfer::encode16(&table, t.linear_max_f32(), v));
        assert_eq!(out, expected);
        let biased =
            color::OPSIN_F32.map(|r| r[0] * l[0] + r[1] * l[1] + r[2] * l[2] + color::OPSIN_BIAS);
        let root_delta = biased
            .map(|v| f64::from(magetypes::nostd_math::cbrt_midp_f32(v)) - f64::from(v).cbrt());
        let ideal_biased = matrix(color::OPSIN_F32.map(|r| r.map(f64::from)), l.map(f64::from))
            .map(|v| v + f64::from(color::OPSIN_BIAS));
        let matrix_delta: [f64; 3] =
            core::array::from_fn(|c| f64::from(biased[c]) - ideal_biased[c]);
        eprintln!(
            "HDRSTAGE {t:?} source={rgb:?} input={l:?} biased_lms={biased:?} matrix_error={matrix_delta:?} root_error={root_delta:?} planes={p:?}"
        );
        evaluate(t, &[rgb], 65535, "retained");
    }
    // Exhaustive scalar transfer audit of every source grid, before color.
    for t in [transfer::Transfer::Pq, transfer::Transfer::Hlg] {
        let table = t.table();
        let mut max_error = 0;
        let mut changed = 0;
        for bits in 1..=16 {
            let max = ((1u32 << bits) - 1) as u16;
            for v in 0..=max {
                let l = t.decode(f64::from(v) / f64::from(max)) as f32;
                let out = transfer::encode_code(&table, t.linear_max_f32(), l, f32::from(max));
                max_error = max_error.max(v.abs_diff(out));
                changed += usize::from(v != out);
                assert_eq!(code(t, decode(t, f64::from(v) / f64::from(max)), max), v);
            }
        }
        eprintln!("HDRTRANSFER {t:?} grids1..16 changed={changed} max={max_error}");
        assert_eq!(max_error, 0);
    }
}

#[test]
#[ignore = "bounded arithmetic lab; invoke explicitly with --ignored --nocapture"]
fn hdr_precision_frozen_grids() {
    for t in [transfer::Transfer::Pq, transfer::Transfer::Hlg] {
        let full = cases(65535, true);
        assert_eq!(full.len(), 533552);
        evaluate(t, &full, 65535, "full16");
        for bits in 1..=16 {
            evaluate(
                t,
                &cases(((1u32 << bits) - 1) as u16, false),
                ((1u32 << bits) - 1) as u16,
                &alloc::format!("bits{bits}"),
            );
        }
    }
    // Optional existing source fixtures; no regridding or metadata conversion.
    if let Ok(path) = std::env::var("IMIJ_HDR_LAB_FIXTURES") {
        for id in ["1228", "1492", "1503", "1546"] {
            let bytes = std::fs::read(alloc::format!("{path}/{id}/source16.rgb16ne")).unwrap();
            assert_eq!(bytes.len(), 512 * 512 * 6);
            let rgb: Vec<_> = bytes
                .as_chunks::<6>()
                .0
                .iter()
                .map(|p| core::array::from_fn(|c| u16::from_ne_bytes([p[2 * c], p[2 * c + 1]])))
                .collect();
            evaluate(transfer::Transfer::Pq, &rgb, 65535, id);
        }
    }
}
