//! Phase 2 of the jpegsize study: 4:2:0 chroma reconstruction.
//!
//! One shared f32 inverse DCT (jpegsize::idct8x8) lifts each JPEG's coded
//! planes from its own quantized coefficients; only the chroma upsampler
//! differs between arms:
//!
//!   a   group-local Catmull-Rom (B=0, C=0.5): each 8x8 chroma block upsamples
//!       independently to its 16x16 luma group, out-of-block taps clamped into
//!       the block. Chroma seams land on every 16-px group edge.
//!   b   plane-wide triangle: libjpeg-turbo h2v2 fancy-upsample weights
//!       (9/16, 3/16, 3/16, 1/16) over the visible chroma plane.
//!   c   plane-wide Catmull-Rom: the same kernel as (a) but crossing block
//!       boundaries, clamped only at the image edge.
//!   d{32,64,128}  unit-local triangle: (b) inside full-width stripes of 32,
//!       64 or 128 output px; edge replication at each unit's top and bottom,
//!       seams only at unit boundaries.
//!
//! Targets: (i) independent installed libjpeg-turbo ISLOW/fancy/integer RGB
//! (zenjpeg's explicit Libjpeg mode matches on the fresh 20-file pilot) and (ii) a
//! source PNG when the manifest pairs one (PNG-derived JPEGs and synthetics).
//! The ISLOW/float-b control reuses libjpeg integer IDCT and colour conversion
//! with floating triangle upsampling. An integer triangle control must match
//! the independent target exactly for every sampled input. Arms f/f-clamp
//! reuse IMI normative AAN reconstruction and output rounding.
//!
//! Correction pricing for arms a and d{32,64,128} against (b) and (i): IMI
//! lossless bytes of the whole-image RGB8 modular delta (target minus arm), of
//! strips of only the differing rows or columns, and of the target's pixels
//! outright in the same strips; plus the differing-row/column counts and a
//! delta-value histogram.
//!
//!   imij-cmp jpegsize-chroma OUT.tsv MANIFEST [--crops DIR]
//!   MANIFEST: one line per input, `FILE.jpg` or `FILE.jpg \t SOURCE.png`.
//!   --crops DIR: per arm, a 64x64 crop around the worst diff vs (i) of the
//!   arm, of (b), and of (i) itself, as RGB PNGs.

use crate::coefjpeg::ZIGZAG;
use crate::jpegsize::{YCBCR_CENTERED, idct8x8, jpeg_head, short};
use crate::rd::Result;
use enough::Unstoppable;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const ARMS: &[&str] = &[
    "a",
    "b",
    "c",
    "d32",
    "d64",
    "d128",
    "islow-float-b",
    "f",
    "f-clamp",
];
const CORR_ARMS: &[&str] = &["a", "d32", "d64", "d128"];
const HEADER: &str = "path\tclass\tw\th\tarm\ttarget\tmax_abs\tmean_abs\tgt1_share\tedge_gt1\tint_gt1\tssim2\tba_max\tcorr_vs\td_full\td_rows\td_cols\tp_rows\tp_cols\tn_rows\tn_cols\thist\tnote\tgt2_share\tchannel_hist\tworst_x\tworst_y\tzenjpeg_max\tzenjpeg_differing_pixels\tsha256\tlocations";

/// Catmull-Rom B=0 C=0.5 weights for output sample r of a 2x upsample:
/// position t = r/2 - 1/4, taps at floor(t)-1 .. floor(t)+2.
fn cr_weights(r: usize) -> (i32, [f32; 4]) {
    let t = r as f32 * 0.5 - 0.25;
    let i = t.floor() as i32;
    let f = t - i as f32;
    let (f2, f3) = (f * f, f * f * f);
    (
        i,
        [
            0.5 * (2.0 * f2 - f3 - f),
            1.0 + 0.5 * (3.0 * f3 - 5.0 * f2),
            0.5 * (4.0 * f2 - 3.0 * f3 + f),
            0.5 * (f3 - f2),
        ],
    )
}

/// One Catmull-Rom output sample along a 1-D run: taps at i-1..i+2 clamped
/// into [0, n).
fn cr_tap<F: Fn(usize) -> f32>(at: F, i: i32, w: [f32; 4], n: usize) -> f32 {
    let hi = n as i32 - 1;
    let mut index = (i - 1).clamp(0, hi) as usize;
    let mut weight = w[0];
    let mut sum = 0.0f32;
    for (k, &wk) in w.iter().enumerate().skip(1) {
        let next = (i - 1 + k as i32).clamp(0, hi) as usize;
        if next == index {
            weight += wk;
        } else {
            sum += weight * at(index);
            index = next;
            weight = wk;
        }
    }
    sum + weight * at(index)
}

/// Plane-wide Catmull-Rom: cw x ch -> 2cw x 2ch, taps clamped at plane edges.
fn up_plane_cr(src: &[f32], cw: usize, ch: usize, stride: usize) -> Vec<f32> {
    let (ow, oh) = (cw * 2, ch * 2);
    let mut row = vec![0f32; ow * ch];
    for y in 0..ch {
        for ox in 0..ow {
            let (i, w) = cr_weights(ox);
            row[y * ow + ox] = cr_tap(|j| src[y * stride + j], i, w, cw);
        }
    }
    let mut out = vec![0f32; ow * oh];
    for oy in 0..oh {
        let (i, w) = cr_weights(oy);
        for ox in 0..ow {
            out[oy * ow + ox] = cr_tap(|j| row[j * ow + ox], i, w, ch);
        }
    }
    out
}

/// Group-local Catmull-Rom: each 8x8 block upsamples to its own 16x16 group.
/// (cw, ch) are block multiples; cr_weights gives block-local tap indices.
fn up_group_cr(src: &[f32], cw: usize, ch: usize, stride: usize) -> Vec<f32> {
    let (bw, bh) = (cw / 8, ch / 8);
    debug_assert_eq!(cw % 8 + ch % 8, 0);
    let mut out = vec![0f32; cw * 2 * ch * 2];
    for by in 0..bh {
        for bx in 0..bw {
            // rows: 8 -> 16 inside the block
            let mut tmp = [0f32; 8 * 16];
            for y in 0..8 {
                for ox in 0..16 {
                    let (i, w) = cr_weights(ox);
                    tmp[y * 16 + ox] = cr_tap(|j| src[(by * 8 + y) * stride + bx * 8 + j], i, w, 8);
                }
            }
            // cols: 8 -> 16 inside the block
            for oy in 0..16 {
                let (i, w) = cr_weights(oy);
                for ox in 0..16 {
                    out[(by * 16 + oy) * cw * 2 + bx * 16 + ox] =
                        cr_tap(|j| tmp[j * 16 + ox], i, w, 8);
                }
            }
        }
    }
    out
}

/// libjpeg-turbo h2v2 fancy upsample with exact 9/16,3/16,3/16,1/16 weights
/// in f32: out[2y] taps (y, clamp(y-1)), out[2y+1] taps (y, clamp(y+1)), and
/// horizontally out[2x] taps (x, x-1), out[2x+1] taps (x, x+1). `unit_rows`
/// bounds the vertical far tap to a stripe's own rows (unit-local mode); pass
/// ch for plane-wide.
fn up_triangle(src: &[f32], cw: usize, ch: usize, unit_rows: usize, stride: usize) -> Vec<f32> {
    let (ow, oh) = (cw * 2, ch * 2);
    let mut out = vec![0f32; ow * oh];
    for oy in 0..oh {
        let ny = oy / 2;
        let u0 = ny / unit_rows * unit_rows;
        let u1 = (u0 + unit_rows).min(ch);
        let fy = (if oy % 2 == 0 {
            ny as i64 - 1
        } else {
            ny as i64 + 1
        })
        .clamp(u0 as i64, u1 as i64 - 1) as usize;
        let (near, far) = (
            &src[ny * stride..ny * stride + cw],
            &src[fy * stride..fy * stride + cw],
        );
        let cs = |x: usize| 3.0 * near[x] + far[x];
        for ox in 0..ow {
            let nx = ox / 2;
            let fx = if ox % 2 == 0 {
                nx.saturating_sub(1)
            } else {
                (nx + 1).min(cw - 1)
            };
            out[oy * ow + ox] = (3.0 * cs(nx) + cs(fx)) * 0.0625;
        }
    }
    out
}

/// Coded-dimensions f32 plane of one component: dequantize zigzag coeffs by
/// the natural-order table, idct8x8 per block, scatter by (bx, by).
fn coef_plane(dec: &zenjpeg::decoder::DecodedCoefficients, ci: usize) -> (Vec<f32>, usize, usize) {
    let comp = &dec.components[ci];
    let tq = dec.quant_tables[comp.quant_table_idx as usize].expect("missing quant table");
    let (bw, bh) = (comp.blocks_wide, comp.blocks_high);
    let mut plane = vec![0f32; bw * 8 * bh * 8];
    for b in 0..comp.num_blocks() {
        let (bx, by) = (b % bw, b / bw);
        let src = comp.block(b);
        let mut deq = [0f32; 64];
        for (k, &v) in src.iter().enumerate() {
            let i = ZIGZAG[k];
            deq[i] = v as f32 * tq[i] as f32;
        }
        let px = idct8x8(&deq);
        for (i, &v) in px.iter().enumerate() {
            plane[(by * 8 + i / 8) * bw * 8 + bx * 8 + i % 8] = v;
        }
    }
    (plane, bw * 8, bh * 8)
}

/// Centered-YCbCr f32 planes -> RGB u8, the same signalled transform the
/// pixel oracle uses.
fn compose_rgb(
    y: &[f32],
    cb: &[f32],
    cr: &[f32],
    w: usize,
    h: usize,
    yw: usize,
    cw2: usize,
) -> Vec<u8> {
    let mut rgb = vec![0u8; w * h * 3];
    for r in 0..h {
        for c in 0..w {
            let p = [y[r * yw + c], cb[r * cw2 + c], cr[r * cw2 + c]];
            for ch in 0..3 {
                let m = YCBCR_CENTERED[ch];
                let v = m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3];
                rgb[(r * w + c) * 3 + ch] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    rgb
}

struct Stats {
    max_abs: u32,
    mean_abs: f64,
    gt1: f64,
    edge_gt1: f64,
    int_gt1: f64,
    worst: (usize, usize, u32),
    gt2: f64,
    channel_hist: [[usize; 6]; 3],
    location_pixels: [usize; 3],
    location_gt2: [[usize; 3]; 3],
}

/// Diff stats of arm vs target. A pixel "differs by >1" when any channel
/// does. Edge = the two rows/cols at each 16-px group edge (y%16 or x%16 in
/// {0,15}); interior = the rest.
fn diffs(
    arm: &[u8],
    tgt: &[u8],
    w: usize,
    h: usize,
    arm_stride: usize,
    target_stride: usize,
) -> Stats {
    let (mut max, mut sum, mut gt1, mut egt1, mut igt1) = (0u32, 0f64, 0usize, 0usize, 0usize);
    let (mut en, mut inn) = (0usize, 0usize);
    let mut worst = (0usize, 0usize, 0u32);
    let mut gt2 = 0usize;
    let mut channel_hist = [[0usize; 6]; 3];
    let mut location_pixels = [0usize; 3];
    let mut location_gt2 = [[0usize; 3]; 3];
    for r in 0..h {
        for c in 0..w {
            let at = r * target_stride + c * 3;
            let saturated = tgt[at..at + 3].iter().any(|&v| v == 0 || v == 255);
            let flat = [(r, (c + 1).min(w - 1)), ((r + 1).min(h - 1), c)]
                .iter()
                .all(|&(rr, cc)| {
                    (0..3)
                        .all(|ch| tgt[at + ch].abs_diff(tgt[rr * target_stride + cc * 3 + ch]) <= 1)
                });
            let categories = [
                r % 16 == 0 || r % 16 == 15 || c % 16 == 0 || c % 16 == 15,
                saturated,
                flat,
            ];
            for i in 0..3 {
                location_pixels[i] += usize::from(categories[i]);
            }
            let mut pm = 0u32;
            for ch in 0..3 {
                let d = (arm[r * arm_stride + c * 3 + ch] as i32
                    - tgt[r * target_stride + c * 3 + ch] as i32)
                    .unsigned_abs();
                channel_hist[ch][match d {
                    0 => 0,
                    1 => 1,
                    2 => 2,
                    3 => 3,
                    4..=7 => 4,
                    _ => 5,
                }] += 1;
                for i in 0..3 {
                    location_gt2[ch][i] += usize::from(d > 2 && categories[i]);
                }
                sum += d as f64;
                pm = pm.max(d);
            }
            gt2 += usize::from(pm > 2);
            if pm > max {
                max = pm;
                worst = (c, r, pm);
            }
            let edge = (r % 16 == 0 || r % 16 == 15) || (c % 16 == 0 || c % 16 == 15);
            if edge {
                en += 1;
            } else {
                inn += 1;
            }
            if pm > 1 {
                gt1 += 1;
                if edge {
                    egt1 += 1;
                } else {
                    igt1 += 1;
                }
            }
        }
    }
    let n = (w * h * 3) as f64;
    Stats {
        max_abs: max,
        mean_abs: sum / n,
        gt1: gt1 as f64 / (w * h) as f64,
        edge_gt1: if en > 0 { egt1 as f64 / en as f64 } else { 0.0 },
        int_gt1: if inn > 0 {
            igt1 as f64 / inn as f64
        } else {
            0.0
        },
        worst,
        gt2: gt2 as f64 / (w * h) as f64,
        channel_hist,
        location_pixels,
        location_gt2,
    }
}

/// SSIMULACRA2 and butteraugli max of `img` against reference `refpx`.
fn quality(
    refpx: &[u8],
    img: &[u8],
    w: usize,
    h: usize,
    reference_stride: usize,
    image_stride: usize,
) -> (String, String) {
    if w * h > 16_000_000 {
        eprintln!("chroma: full SSIMULACRA2 {w}x{h}");
    }
    let s2 = (|| -> Result<f64> {
        // PixelSlice expresses its stride in whole pixels. Preserve arbitrary
        // byte padding at this boundary; the usual RGB stride stays borrowed.
        fn representable(
            px: &[u8],
            stride: usize,
            w: usize,
            h: usize,
        ) -> (std::borrow::Cow<'_, [u8]>, usize) {
            if stride % 3 == 0 {
                (std::borrow::Cow::Borrowed(px), stride)
            } else {
                (
                    std::borrow::Cow::Owned(visible(px, stride, w * 3, h)),
                    w * 3,
                )
            }
        }
        let (reference, reference_stride) = representable(refpx, reference_stride, w, h);
        let (image, image_stride) = representable(img, image_stride, w, h);
        let r = fast_ssim2::PixelSlice::new(
            reference.as_ref(),
            w as u32,
            h as u32,
            reference_stride,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )?;
        let d = fast_ssim2::PixelSlice::new(
            image.as_ref(),
            w as u32,
            h as u32,
            image_stride,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )?;
        // Full-image metric, without retaining a reference pyramid between arms.
        let evaluate = || fast_ssim2::compute_ssimulacra2(&r, &d);
        Ok(if w * h > 16_000_000 {
            // Limit simultaneous full-plane Gaussian scratch allocations.
            static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
            POOL.get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .expect("metric thread pool")
            })
            .install(evaluate)?
        } else {
            evaluate()?
        })
    })();
    let s2s = match s2 {
        Ok(v) => format!("{v:.4}"),
        Err(e) => format!("err:{}", short(&e.to_string())),
    };
    if w * h > 16_000_000 {
        eprintln!("chroma: SSIMULACRA2 {s2s}; Butteraugli {w}x{h}");
    }
    let ba = (|| -> Result<f64> {
        let rgb = |px: &[u8], stride: usize| -> Vec<rgb::Rgb<u8>> {
            (0..h)
                .flat_map(|row| px[row * stride..row * stride + w * 3].chunks_exact(3))
                .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
                .collect()
        };
        let reference = rgb(refpx, reference_stride);
        let image = rgb(img, image_stride);
        Ok(butteraugli::butteraugli_strip(
            imgref::ImgRef::new(&reference, w, h),
            imgref::ImgRef::new(&image, w, h),
            &butteraugli::ButteraugliParams::default(),
            256,
        )?
        .score)
    })();
    let bas = match ba {
        Ok(v) => format!("{v:.4}"),
        Err(e) => format!("err:{}", short(&e.to_string())),
    };
    (s2s, bas)
}

/// Full-image metric worker: reconstruction and correction buffers are absent.
pub(crate) fn metrics(args: &[String]) -> Result<()> {
    if args.len() != 4 {
        return Err("usage: imij-cmp jpegsize-metrics TARGET.rgb8 ARM.rgb8 WIDTH HEIGHT".into());
    }
    let (w, h): (usize, usize) = (args[2].parse()?, args[3].parse()?);
    let reference = fs::read(&args[0])?;
    let image = fs::read(&args[1])?;
    if reference.len() != w * h * 3 || image.len() != reference.len() {
        return Err("metric pixel geometry differs".into());
    }
    let (s, b) = quality(&reference, &image, w, h, w * 3, w * 3);
    if s.starts_with("err") || b.starts_with("err") {
        return Err(format!("metric failed: {s} {b}").into());
    }
    println!("{s}\t{b}");
    Ok(())
}

pub(crate) fn correction_worker(args: &[String]) -> Result<()> {
    if args.len() != 4 {
        return Err("usage: imij-cmp jpegsize-correction TARGET.rgb8 ARM.rgb8 WIDTH HEIGHT".into());
    }
    let (w, h): (usize, usize) = (args[2].parse()?, args[3].parse()?);
    let reference = fs::read(&args[0])?;
    let image = fs::read(&args[1])?;
    if reference.len() != w * h * 3 || image.len() != reference.len() {
        return Err("correction pixel geometry differs".into());
    }
    println!(
        "{}",
        serde_json::to_string(&correction(&image, &reference, w, h, w * 3, w * 3)?)?
    );
    Ok(())
}

fn correction_files(reference: &Path, image: &Path, w: usize, h: usize) -> Result<Corr> {
    let result = std::process::Command::new(std::env::current_exe()?)
        .arg("jpegsize-correction")
        .arg(reference)
        .arg(image)
        .arg(w.to_string())
        .arg(h.to_string())
        .stderr(std::process::Stdio::inherit())
        .output()?;
    if !result.status.success() {
        return Err(format!("correction worker failed: {}", result.status).into());
    }
    Ok(serde_json::from_slice(&result.stdout)?)
}

fn quality_files(reference: &Path, image: &Path, w: usize, h: usize) -> Result<(String, String)> {
    let result = std::process::Command::new(std::env::current_exe()?)
        .arg("jpegsize-metrics")
        .arg(reference)
        .arg(image)
        .arg(w.to_string())
        .arg(h.to_string())
        .stderr(std::process::Stdio::inherit())
        .output()?;
    if !result.status.success() {
        return Err(format!("full-image metric worker failed: {}", result.status).into());
    }
    let output = String::from_utf8(result.stdout)?;
    let parts: Vec<_> = output.trim().split('\t').collect();
    if parts.len() != 2 {
        return Err("metric worker output differs".into());
    }
    Ok((parts[0].to_owned(), parts[1].to_owned()))
}

/// IMI lossless byte count of an RGB8 buffer.
fn imij_ll(px: &[u8], w: usize, h: usize, stride: usize) -> Result<usize> {
    let info = imij::ImageInfo::new(
        w as u32,
        h as u32,
        imij::Channels::Rgb,
        imij::SampleType::U8,
    );
    if w * h > 16_000_000 {
        eprintln!("chroma: correction lossless encode {w}x{h}");
    }
    let file = imij::ImiEncoderConfig::default().encode_samples(
        imij::ImageRef::new(px, stride, &info),
        &imij::Metadata::default(),
    )?;
    if w * h > 16_000_000 {
        eprintln!("chroma: correction lossless decode {w}x{h}");
    }
    let (decoded_info, decoded) = imij::decode(&file)?;
    if decoded_info != info
        || decoded.len() != w * h * 3
        || (0..h).any(|r| decoded[r * w * 3..(r + 1) * w * 3] != px[r * stride..r * stride + w * 3])
    {
        return Err("correction image lossless round-trip mismatch".into());
    }
    if let Some(dir) = std::env::var_os("JPEG_CHROMA_ARTIFACTS") {
        use sha2::{Digest, Sha256};
        let path = PathBuf::from(dir);
        fs::create_dir_all(&path)?;
        fs::write(path.join(format!("{:x}.imi", Sha256::digest(&file))), &file)?;
    }
    Ok(file.len())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Corr {
    d_full: usize,
    d_rows: usize,
    d_cols: usize,
    p_rows: usize,
    p_cols: usize,
    n_rows: usize,
    n_cols: usize,
    hist: String,
}

/// A bitmap identifies the original rows/columns of a packed strip. Price
/// its lossless file plus two u64 frame lengths; the base image supplies
/// geometry and the selected strip representation supplies the axis.
fn index_cost(selected: &[bool]) -> Result<usize> {
    let mut bits = vec![0u8; selected.len().div_ceil(8)];
    for (i, &yes) in selected.iter().enumerate() {
        bits[i / 8] |= u8::from(yes) << (i % 8);
    }
    let info = imij::ImageInfo::new(
        bits.len() as u32,
        1,
        imij::Channels::Gray,
        imij::SampleType::U8,
    );
    let file = imij::ImiEncoderConfig::default().encode_samples(
        imij::ImageRef::new(&bits, bits.len(), &info),
        &imij::Metadata::default(),
    )?;
    if imij::decode(&file)? != (info, bits) {
        return Err("strip index round-trip mismatch".into());
    }
    Ok(file.len() + 16)
}

/// Correction-layer pricing: the reversible modular RGB8 delta as a
/// whole image; the same delta packed into strips of only the differing rows
/// and, separately, differing columns; the target pixels outright in the same
/// strips; plus the counts and complete signed delta-value histogram.
/// Given the arm, wrapping addition recovers every possible RGB8 target.
fn correction(
    arm: &[u8],
    tgt: &[u8],
    w: usize,
    h: usize,
    arm_stride: usize,
    target_stride: usize,
) -> Result<Corr> {
    let mut delta = vec![0u8; w * h * 3];
    let (mut rows, mut cols) = (vec![false; h], vec![false; w]);
    let mut hist: BTreeMap<i32, usize> = BTreeMap::new();
    for r in 0..h {
        for c in 0..w {
            let mut any = false;
            for ch in 0..3 {
                let d = tgt[r * target_stride + c * 3 + ch] as i32
                    - arm[r * arm_stride + c * 3 + ch] as i32;
                delta[(r * w + c) * 3 + ch] = tgt[r * target_stride + c * 3 + ch]
                    .wrapping_sub(arm[r * arm_stride + c * 3 + ch]);
                *hist.entry(d).or_default() += 1;
                any |= d != 0;
            }
            if any {
                rows[r] = true;
                cols[c] = true;
            }
        }
    }
    if (0..h).any(|r| {
        (0..w * 3).any(|c| {
            arm[r * arm_stride + c].wrapping_add(delta[r * w * 3 + c]) != tgt[r * target_stride + c]
        })
    }) {
        return Err("modular correction does not recover target".into());
    }
    let ridx: Vec<usize> = (0..h).filter(|&r| rows[r]).collect();
    let cidx: Vec<usize> = (0..w).filter(|&c| cols[c]).collect();
    let pick = |src: &[u8], stride: usize, rs: &[usize], cs: &[usize]| -> Vec<u8> {
        let mut v = Vec::with_capacity(rs.len() * cs.len() * 3);
        for &r in rs {
            for &c in cs {
                v.extend_from_slice(&src[r * stride + c * 3..r * stride + c * 3 + 3]);
            }
        }
        v
    };
    let allr: Vec<usize> = (0..h).collect();
    let allc: Vec<usize> = (0..w).collect();
    let d_full = imij_ll(&delta, w, h, w * 3)?;
    let row_index = if ridx.is_empty() {
        0
    } else {
        index_cost(&rows)?
    };
    let col_index = if cidx.is_empty() {
        0
    } else {
        index_cost(&cols)?
    };
    // Encode one packed strip at a time; do not retain four full-size copies.
    let price =
        |src: &[u8], stride: usize, rs: &[usize], cs: &[usize], index: usize| -> Result<usize> {
            if rs.is_empty() || cs.is_empty() {
                return Ok(0);
            }
            imij_ll(&pick(src, stride, rs, cs), cs.len(), rs.len(), cs.len() * 3)
                .map(|bytes| bytes + index)
        };
    let d_rows = price(&delta, w * 3, &ridx, &allc, row_index)?;
    let d_cols = price(&delta, w * 3, &allr, &cidx, col_index)?;
    let p_rows = price(tgt, target_stride, &ridx, &allc, row_index)?;
    let p_cols = price(tgt, target_stride, &allr, &cidx, col_index)?;
    let mut hs = String::new();
    for (v, n) in hist {
        let _ = write!(hs, "{v}:{n},");
    }
    Ok(Corr {
        d_full,
        d_rows,
        d_cols,
        p_rows,
        p_cols,
        n_rows: ridx.len(),
        n_cols: cidx.len(),
        hist: hs.trim_end_matches(',').into(),
    })
}

fn crop(
    px: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    cx: usize,
    cy: usize,
    side: usize,
) -> (Vec<rgb::Rgb<u8>>, usize, usize) {
    let x0 = cx.saturating_sub(side / 2).min(w.saturating_sub(side));
    let y0 = cy.saturating_sub(side / 2).min(h.saturating_sub(side));
    let (cw, ch) = (side.min(w), side.min(h));
    let mut v = Vec::with_capacity(cw * ch);
    for r in y0..y0 + ch {
        for c in x0..x0 + cw {
            let i = r * stride + c * 3;
            v.push(rgb::Rgb::new(px[i], px[i + 1], px[i + 2]));
        }
    }
    (v, cw, ch)
}

fn write_png(dir: &Path, name: &str, buf: Vec<rgb::Rgb<u8>>, w: usize, h: usize) -> Result<()> {
    fs::create_dir_all(dir)?;
    let img = imgref::ImgRef::new(&buf, w, h);
    let mut cfg = zenpng::EncodeConfig::default();
    cfg.srgb_intent = Some(0);
    // The stats entry point keeps truecolor for indexed candidates.
    let (png, _) = zenpng::encode_rgb8_with_stats(img, None, &cfg, &Unstoppable, &Unstoppable)?;
    fs::write(dir.join(name), png)?;
    Ok(())
}

fn is_420(h: &crate::jpegsize::Head) -> bool {
    h.ncomp == 3
        && h.comps.len() == 3
        && h.comps[0].1 == 2
        && h.comps[0].2 == 2
        && h.comps[1].1 == 1
        && h.comps[1].2 == 1
        && h.comps[2].1 == 1
        && h.comps[2].2 == 1
        && h.precision == 8
}

fn decode_rgb(data: &[u8]) -> Result<Vec<u8>> {
    let mut dc = zenjpeg::decoder::DecodeConfig::new()
        .orientation(zenjpeg::decode::OrientationHint::Preserve)
        .idct_method(zenjpeg::decode::IdctMethod::Libjpeg)
        .max_memory(2 * 1024 * 1024 * 1024);
    dc.output_format = Some(zenjpeg::decoder::PixelFormat::Rgb);
    let res = dc.decode(data, Unstoppable)?;
    res.pixels_u8()
        .map(|p| p.to_vec())
        .ok_or_else(|| "no u8 pixels".into())
}

fn png_rgb(img: &zenpng::PngDecodeOutput) -> Result<Vec<u8>> {
    if img.info.bit_depth != 8 {
        return Err("PNG not 8-bit".into());
    }
    let px = img.pixels.copy_to_contiguous_bytes();
    let n = img.info.width as usize * img.info.height as usize;
    match px.len().checked_div(n).ok_or("empty PNG")? {
        3 => Ok(px),
        4 => {
            if px.chunks_exact(4).any(|c| c[3] != 255) {
                return Err("paired PNG has nonopaque alpha".into());
            }
            Ok(px
                .chunks_exact(4)
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect())
        }
        1 => Ok(px.iter().flat_map(|&v| [v, v, v]).collect()),
        _ => Err("PNG unsupported sample layout".into()),
    }
}

fn require_srgb(info: &zenpng::PngInfo) -> Result<()> {
    use zenpixels_convert::icc::{IdentificationUse, identify_common};
    if let Some(icc) = &info.icc_profile {
        let id = identify_common(icc).ok_or("paired PNG ICC needs explicit colour conversion")?;
        if !id.is_srgb() || id.valid_use != IdentificationUse::MatrixTrcSubstitution {
            return Err("paired PNG ICC needs explicit sRGB conversion".into());
        }
    }
    if info
        .cicp
        .is_some_and(|c| c != zenpixels_convert::Cicp::SRGB)
    {
        return Err("paired PNG CICP is not sRGB".into());
    }
    // The study accepts explicit sRGB, recognized sRGB ICC, or untagged RGB.
    // Other gamma/chromaticity-only interpretations require conversion first.
    if info.srgb_intent.is_none()
        && info.icc_profile.is_none()
        && info.cicp.is_none()
        && (info.source_gamma.is_some() || info.chromaticities.is_some())
    {
        return Err("paired PNG gamma/chromaticities need explicit sRGB conversion".into());
    }
    Ok(())
}

fn decode_png(p: &Path) -> Result<(Vec<u8>, usize, usize)> {
    let img = zenpng::decode(
        &fs::read(p)?,
        &zenpng::PngDecodeConfig::default(),
        &Unstoppable,
    )?;
    require_srgb(&img.info)?;
    Ok((
        png_rgb(&img)?,
        img.info.width as usize,
        img.info.height as usize,
    ))
}

/// Convert embedded photo ICC profiles through the installed CMS to RGB8 sRGB before
/// producing the supplemental q50/75/85/95 pairs. Original files are untouched.
pub(crate) fn source_pairs(args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err("usage: imij-cmp jpegsize-source-pairs NEW_DIRECTORY PAIRED_MANIFEST".into());
    }
    let oracle = std::env::var_os("JPEG_COLOUR_ORACLE").ok_or("JPEG_COLOUR_ORACLE is required")?;
    let dir = Path::new(&args[0]);
    fs::create_dir(dir)?;
    let mut sources = BTreeMap::new();
    for line in fs::read_to_string(&args[1])?
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (_, png) = line.split_once('\t').ok_or("pair lacks PNG")?;
        let bytes = fs::read(png)?;
        sources
            .entry(format!("{:x}", Sha256::digest(&bytes)))
            .or_insert(bytes);
    }
    let mut manifest = fs::File::create(dir.join("manifest.txt"))?;
    let mut provenance = fs::File::create(dir.join("sources.jsonl"))?;
    for (sha, bytes) in sources {
        let img = zenpng::decode(&bytes, &zenpng::PngDecodeConfig::default(), &Unstoppable)?;
        let icc = img
            .info
            .icc_profile
            .as_ref()
            .ok_or("conversion source lacks ICC")?;
        let w = img.info.width as usize;
        let h = img.info.height as usize;
        let input = png_rgb(&img)?;
        let iccpath = dir.join(format!("{sha}.icc"));
        let rawpath = dir.join(format!("{sha}_source.rgb8"));
        let convertedpath = dir.join(format!("{sha}_srgb.rgb8"));
        fs::write(&iccpath, icc)?;
        fs::write(&rawpath, &input)?;
        let conversion = std::process::Command::new(&oracle)
            .args([
                iccpath.as_os_str(),
                rawpath.as_os_str(),
                convertedpath.as_os_str(),
            ])
            .args([w.to_string(), h.to_string()])
            .output()?;
        if !conversion.status.success() {
            return Err(format!(
                "installed CMS conversion failed: {}",
                String::from_utf8_lossy(&conversion.stderr)
            )
            .into());
        }
        let cms_version: u32 = std::str::from_utf8(&conversion.stdout)?.trim().parse()?;
        let output = fs::read(convertedpath)?;
        if output.len() != input.len() {
            return Err("CMS output geometry changed".into());
        }
        let changed = input.iter().zip(&output).filter(|(a, b)| a != b).count();
        let pixelsha = format!("{:x}", Sha256::digest(&output));
        let pngpath = dir.join(format!("{sha}_srgb.png"));
        let pixels = output
            .chunks_exact(3)
            .map(|c| rgb::Rgb::new(c[0], c[1], c[2]))
            .collect();
        write_png(
            dir,
            pngpath
                .file_name()
                .unwrap()
                .to_str()
                .ok_or("PNG filename")?,
            pixels,
            w,
            h,
        )?;
        let (roundtrip, rw, rh) = decode_png(&pngpath)?;
        if rw != w || rh != h || format!("{:x}", Sha256::digest(&roundtrip)) != pixelsha {
            return Err("converted PNG pixels failed exact round trip".into());
        }
        let pngsha = format!("{:x}", Sha256::digest(fs::read(&pngpath)?));
        for q in [50, 75, 85, 95] {
            let jpgpath = dir.join(format!("{sha}_q{q}.jpg"));
            zenenc(&[
                jpgpath.to_string_lossy().into_owned(),
                pngpath.to_string_lossy().into_owned(),
                q.to_string(),
            ])?;
            let jpegsha = format!("{:x}", Sha256::digest(fs::read(&jpgpath)?));
            writeln!(manifest, "{}\t{}", jpgpath.display(), pngpath.display())?;
            writeln!(
                provenance,
                "{}",
                serde_json::json!({"class":"photo_png","pattern":format!("photo_{sha}"),"quality":q,"jpeg_sha256":jpegsha,"png_sha256":pngsha,"original_png_sha256":sha,"original_icc_sha256":format!("{:x}",Sha256::digest(icc)),"converted_rgb_sha256":pixelsha,"colour_policy":"LittleCMS embedded ICC to RGB8 sRGB, relative colorimetric, no black-point compensation","lcms_encoded_version":cms_version,"colour_oracle_sha256":format!("{:x}",Sha256::digest(fs::read(&oracle)?)),"changed_channel_samples":changed,"width":w,"height":h})
            )?;
        }
    }
    Ok(())
}

/// Compact only visible samples; coded padding is retained solely for arm a.
fn visible<T: Copy>(p: &[T], stride: usize, w: usize, h: usize) -> Vec<T> {
    (0..h)
        .flat_map(|y| p[y * stride..y * stride + w].iter().copied())
        .collect()
}

fn normative_plane(
    dec: &zenjpeg::decoder::DecodedCoefficients,
    ci: usize,
    clamp: bool,
) -> (Vec<f32>, usize) {
    let comp = &dec.components[ci];
    let table = dec.quant_tables[comp.quant_table_idx as usize].expect("table");
    let stride = comp.blocks_wide * 8;
    let mut out = vec![0.0; stride * comp.blocks_high * 8];
    for b in 0..comp.num_blocks() {
        let mut q = [0i16; 64];
        for (k, &v) in comp.block(b).iter().enumerate() {
            q[ZIGZAG[k]] = v;
        }
        let px = imij::lab::jpeg_idct(&q, &table);
        for (i, &v) in px.iter().enumerate() {
            out[(b / comp.blocks_wide * 8 + i / 8) * stride + b % comp.blocks_wide * 8 + i % 8] =
                if clamp { v.clamp(-128.0, 127.0) } else { v };
        }
    }
    (out, stride)
}

fn islow_plane(dec: &zenjpeg::decoder::DecodedCoefficients, ci: usize) -> (Vec<i16>, usize) {
    let comp = &dec.components[ci];
    let table = dec.quant_tables[comp.quant_table_idx as usize].expect("table");
    let stride = comp.blocks_wide * 8;
    let mut out = vec![0i16; stride * comp.blocks_high * 8];
    for b in 0..comp.num_blocks() {
        let mut q = [0i32; 64];
        for (k, &v) in comp.block(b).iter().enumerate() {
            q[ZIGZAG[k]] = i32::from(v) * i32::from(table[ZIGZAG[k]]);
        }
        let mut px = [0i16; 64];
        zenjpeg::decode::idct_int::idct_int_libjpeg(&mut q, &mut px, 8);
        for (i, &v) in px.iter().enumerate() {
            out[(b / comp.blocks_wide * 8 + i / 8) * stride + b % comp.blocks_wide * 8 + i % 8] = v;
        }
    }
    (out, stride)
}

fn up_integer(src: &[i16], cw: usize, ch: usize, stride: usize, w: usize, h: usize) -> Vec<i16> {
    use zenjpeg::decode::upsample::{H2v2Bias, upsample_h2v2_libjpeg_row};
    let mut out = vec![0; w * h];
    for y in 0..h {
        let near = y / 2;
        let far = if y % 2 == 0 {
            near.saturating_sub(1)
        } else {
            (near + 1).min(ch - 1)
        };
        upsample_h2v2_libjpeg_row(
            &src[near * stride..near * stride + cw],
            &src[far * stride..far * stride + cw],
            &mut out[y * w..(y + 1) * w],
            cw,
            w,
            H2v2Bias::Turbo,
        );
    }
    out
}

fn exact_target(path: &Path, n: usize) -> Result<Vec<u8>> {
    let oracle = std::env::var_os("JPEG_PIXEL_ORACLE").ok_or("JPEG_PIXEL_ORACLE is required")?;
    let r = std::process::Command::new(oracle).arg(path).output()?;
    if !r.status.success() {
        return Err(format!(
            "pixel oracle failed: {}",
            String::from_utf8_lossy(&r.stderr)
        )
        .into());
    }
    if r.stdout.len() != n {
        return Err("pixel oracle geometry mismatch".into());
    }
    Ok(r.stdout)
}

fn reconstruct_f(
    dec: &zenjpeg::decoder::DecodedCoefficients,
    w: usize,
    h: usize,
    sampled: bool,
    clamp: bool,
) -> Vec<u8> {
    let mut p = Vec::new();
    for ci in 0..dec.components.len() {
        let (plane, stride) = normative_plane(dec, ci, clamp);
        let (cw, ch) = if ci > 0 && sampled {
            (w.div_ceil(2), h.div_ceil(2))
        } else {
            (w, h)
        };
        let v = visible(&plane, stride, cw, ch);
        p.push(if ci > 0 && sampled {
            zenjpeg::decode::upsample::upsample_libjpeg_f32(&v, cw, ch, w, h, 2, 2)
        } else {
            v
        });
    }
    let mut out = vec![0; w * h * 3];
    for i in 0..w * h {
        let rgb = if p.len() == 1 {
            imij::lab::linear_rgb(&[[1.0, 0.0, 0.0, 128.0]; 3], [p[0][i], 0.0, 0.0])
        } else {
            imij::lab::linear_rgb(&YCBCR_CENTERED, [p[0][i], p[1][i], p[2][i]])
        };
        out[i * 3..i * 3 + 3].copy_from_slice(&rgb);
    }
    out
}

/// Persistent content-addressed pixel artifacts allow reconstruction buffers
/// to be released before full-image metrics and correction coding.
fn stage_pixels(px: &[u8], w: usize, h: usize) -> Result<PathBuf> {
    let root = std::env::var_os("JPEG_CHROMA_PIXELS").ok_or("JPEG_CHROMA_PIXELS is required")?;
    let root = PathBuf::from(root);
    fs::create_dir_all(&root)?;
    let digest = format!("{:x}", Sha256::digest(px));
    let path = root.join(format!("{digest}_{w}_{h}.rgb8"));
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
    {
        Ok(mut file) => file.write_all(px)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            use std::io::Read;
            let mut file = fs::File::open(&path)?;
            let mut hash = Sha256::new();
            let mut buf = vec![0; 1024 * 1024];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
            }
            if file.metadata()?.len() != px.len() as u64
                || format!("{:x}", hash.finalize()) != digest
            {
                return Err("existing pixel artifact differs".into());
            }
        }
        Err(e) => return Err(e.into()),
    }
    Ok(path)
}

fn measure_inner(
    path: &Path,
    png: Option<&Path>,
    crops: Option<&Path>,
    rows: &mut fs::File,
    pixels_only: bool,
) -> Result<()> {
    let data = fs::read(path)?;
    let sha = format!("{:x}", Sha256::digest(&data));
    let head = jpeg_head(&data)?;
    let sampled = is_420(&head);
    let class = if sampled {
        "420"
    } else if head.ncomp == 1 {
        "gray"
    } else {
        "444"
    };
    if !sampled
        && !(head.precision == 8
            && (head.ncomp == 1
                || (head.ncomp == 3 && head.comps.iter().all(|c| c.1 == 1 && c.2 == 1))))
    {
        return Err("manifest includes unsupported coefficient geometry".into());
    }
    let dec =
        zenjpeg::decoder::DecodeConfig::new().decode_coefficients(&head.stripped, Unstoppable)?;
    let (w, h) = (head.width as usize, head.height as usize);
    let ti = exact_target(path, w * h * 3)?;
    let zen = decode_rgb(&data)?;
    if zen.len() != ti.len() {
        return Err("zenjpeg target geometry mismatch".into());
    }
    let zmax = zen
        .iter()
        .zip(&ti)
        .map(|(&a, &b)| a.abs_diff(b))
        .max()
        .unwrap_or(0);
    let zcount = zen
        .chunks_exact(3)
        .zip(ti.chunks_exact(3))
        .filter(|(a, b)| a != b)
        .count();
    drop(zen);
    let source = png.map(decode_png).transpose()?;
    if source
        .as_ref()
        .is_some_and(|(_, sw, sh)| (*sw, *sh) != (w, h))
    {
        return Err("source geometry mismatch".into());
    }
    let (yp, yw, _) = coef_plane(&dec, 0);
    let mut cbp = Vec::new();
    let mut crp = Vec::new();
    let mut coded_cw = 0;
    let mut coded_ch = 0;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut cb = Vec::new();
    let mut cr = Vec::new();
    let mut b_img = Vec::new();
    let mut float_control = Vec::new();
    if sampled {
        (cbp, coded_cw, coded_ch) = coef_plane(&dec, 1);
        (crp, _, _) = coef_plane(&dec, 2);
        cb = visible(&cbp, coded_cw, cw, ch);
        cr = visible(&crp, coded_cw, cw, ch);
        b_img = compose_rgb(
            &yp,
            &up_triangle(&cb, cw, ch, ch, cw),
            &up_triangle(&cr, cw, ch, ch, cw),
            w,
            h,
            yw,
            cw * 2,
        );
        let mut ints = Vec::new();
        for ci in 0..3 {
            let (p, s) = islow_plane(&dec, ci);
            ints.push(visible(
                &p,
                s,
                if ci == 0 { w } else { cw },
                if ci == 0 { h } else { ch },
            ));
        }
        let mut exact = vec![0; w * h * 3];
        zenjpeg::color::ycbcr_planes_i16_to_rgb_u8(
            &ints[0],
            &up_integer(&ints[1], cw, ch, cw, w, h),
            &up_integer(&ints[2], cw, ch, cw, w, h),
            &mut exact,
            true,
        );
        if exact != ti {
            return Err(format!("ISLOW/integer control differs from libjpeg on {sha}").into());
        }
        let lift = |p: &[i16]| -> Vec<i16> {
            let input: Vec<f32> = p.iter().map(|&v| f32::from(v)).collect();
            zenjpeg::decode::upsample::upsample_libjpeg_f32(&input, cw, ch, w, h, 2, 2)
                .iter()
                .map(|v| v.round() as i16)
                .collect()
        };
        float_control = vec![0; w * h * 3];
        zenjpeg::color::ycbcr_planes_i16_to_rgb_u8(
            &ints[0],
            &lift(&ints[1]),
            &lift(&ints[2]),
            &mut float_control,
            true,
        );
    }
    let mut staged = Vec::new();
    let target_path = stage_pixels(&ti, w, h)?;
    let source_path = source
        .as_ref()
        .map(|s| stage_pixels(&s.0, w, h))
        .transpose()?;
    for &name in ARMS {
        if !sampled && !name.starts_with('f') {
            continue;
        }
        let img = if name == "b" {
            b_img.clone()
        } else if name == "islow-float-b" {
            float_control.clone()
        } else if name.starts_with('f') {
            reconstruct_f(&dec, w, h, sampled, name == "f-clamp")
        } else {
            let up = |p: &[f32], coded: &[f32]| match name {
                "a" => up_group_cr(coded, coded_cw, coded_ch, coded_cw),
                "c" => up_plane_cr(p, cw, ch, cw),
                "d32" => up_triangle(p, cw, ch, 16, cw),
                "d64" => up_triangle(p, cw, ch, 32, cw),
                "d128" => up_triangle(p, cw, ch, 64, cw),
                _ => unreachable!(),
            };
            compose_rgb(
                &yp,
                &up(&cb, &cbp),
                &up(&cr, &crp),
                w,
                h,
                yw,
                if name == "a" { coded_cw * 2 } else { cw * 2 },
            )
        };
        staged.push((name, stage_pixels(&img, w, h)?));
    }
    let basename = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
    let index = serde_json::json!({"input_sha256":sha,"width":w,"height":h,"sampling":class,"interpolation_profile":"coalesced-clamp",
        "target_i":basename(&target_path),"source_png":source_path.as_deref().map(basename),
        "arms":staged.iter().map(|(name,p)|((*name).to_string(),basename(p))).collect::<BTreeMap<_,_>>(),
        "zenjpeg_max":zmax,"zenjpeg_differing_pixels":zcount});
    let text = serde_json::to_vec_pretty(&index)?;
    let root = PathBuf::from(std::env::var_os("JPEG_CHROMA_PIXELS").unwrap());
    let indexes = root.join("indexes");
    fs::create_dir_all(&indexes)?;
    let index_path = indexes.join(format!("{sha}_{:x}.json", Sha256::digest(&text)));
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(index_path)
    {
        Ok(mut file) => file.write_all(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }

    let b_path = staged
        .iter()
        .find(|p| p.0 == "b")
        .map(|p| p.1.clone())
        .unwrap_or_else(|| target_path.clone());
    drop((dec, yp, cbp, crp, cb, cr, float_control, ti, b_img, source));
    if pixels_only {
        return Ok(());
    }
    for (name, path_pixels) in staged {
        for (tname, target_pixels) in [("i", Some(&target_path)), ("ii", source_path.as_ref())] {
            let Some(target_pixels) = target_pixels else {
                continue;
            };
            let s = {
                let img = fs::read(&path_pixels)?;
                let target = fs::read(target_pixels)?;
                diffs(&img, &target, w, h, w * 3, w * 3)
            };
            eprintln!("chroma: {sha} {name} metric {tname}");
            let (s2, ba) = quality_files(target_pixels, &path_pixels, w, h)?;
            rows.write_all(format!("{}\t{class}\t{w}\t{h}\t{name}\t{tname}\t{}\t{:.8}\t{:.8}\t{:.8}\t{:.8}\t{s2}\t{ba}\t\t\t\t\t\t\t\t\t\t\t{:.8}\t{}\t{}\t{}\t{zmax}\t{zcount}\t{sha}\t{}\n",path.display(),s.max_abs,s.mean_abs,s.gt1,s.edge_gt1,s.int_gt1,s.gt2,serde_json::to_string(&s.channel_hist)?,s.worst.0,s.worst.1,serde_json::to_string(&serde_json::json!({"categories":["grid_edge","saturated","flat"],"pixels":s.location_pixels,"gt2_by_channel":s.location_gt2}))?).as_bytes())?;
            rows.flush()?;
            if CORR_ARMS.contains(&name) && tname == "i" {
                for (vs, target_file) in [("b", &b_path), ("i", &target_path)] {
                    eprintln!("chroma: {sha} {name} correction {vs}");
                    let c = correction_files(target_file, &path_pixels, w, h)?;
                    rows.write_all(format!("{}\t{class}\t{w}\t{h}\t{name}\tcorr-vs-{vs}\t\t\t\t\t\t\t\t{vs}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t\t\t\t\t\t{zmax}\t{zcount}\t{sha}\t\n",path.display(),c.d_full,c.d_rows,c.d_cols,c.p_rows,c.p_cols,c.n_rows,c.n_cols,c.hist).as_bytes())?;
                    rows.flush()?;
                }
            }
            if let Some(dir) = crops {
                if tname == "i" {
                    for (tag, file) in [("arm", &path_pixels), ("b", &b_path), ("i", &target_path)]
                    {
                        let px = fs::read(file)?;
                        let (buf, cropw, croph) = crop(&px, w, h, w * 3, s.worst.0, s.worst.1, 64);
                        write_png(dir, &format!("{sha}_{name}_{tag}.png"), buf, cropw, croph)?;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Encode a PNG source to a 4:2:0 JPEG with zenjpeg (phase-2 synthetic
/// inputs): `imij-cmp jpegsize-zenenc OUT.jpg IN.png QUALITY [444]`.
/// QUALITY is zenjpeg's generic quality (libjpeg-turbo scale).
pub(crate) fn zenenc(args: &[String]) -> Result<()> {
    if args.len() < 3 || args.len() > 4 {
        return Err("usage: imij-cmp jpegsize-zenenc OUT.jpg IN.png QUALITY [444]".into());
    }
    let (px, w, h) = decode_png(Path::new(&args[1]))?;
    let q: f32 = args[2].parse().map_err(|_| "QUALITY must be a number")?;
    let ss = match args.get(3).map(String::as_str) {
        Some("444") => zenjpeg::encode::ChromaSubsampling::None,
        _ => zenjpeg::encode::ChromaSubsampling::Quarter,
    };
    let cfg = zenjpeg::encode::EncoderConfig::ycbcr(q, ss);
    let jpg = cfg
        .encode_bytes(
            &px,
            w as u32,
            h as u32,
            zenjpeg::encode::PixelLayout::Rgb8Srgb,
        )
        .map_err(|e| format!("zenjpeg encode: {e}"))?;
    fs::write(&args[0], jpg)?;
    Ok(())
}

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut pos: Vec<&String> = Vec::new();
    let mut crops: Option<PathBuf> = None;
    let mut pixels_only = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--pixels-only" {
            pixels_only = true;
            i += 1;
        } else if args[i] == "--crops" {
            crops = Some(PathBuf::from(args.get(i + 1).ok_or("--crops needs DIR")?));
            i += 2;
        } else {
            pos.push(&args[i]);
            i += 1;
        }
    }
    if pos.len() != 2 {
        return Err(
            "usage: imij-cmp jpegsize-chroma OUT.tsv MANIFEST [--crops DIR]\n\
             MANIFEST lines: FILE.jpg [\\t SOURCE.png]"
                .into(),
        );
    }
    std::env::var_os("JPEG_CHROMA_PIXELS").ok_or("JPEG_CHROMA_PIXELS is required")?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(pos[0])?;
    writeln!(output, "{HEADER}")?;
    let mut n = 0usize;
    for line in fs::read_to_string(pos[1])?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let jpg = PathBuf::from(it.next().unwrap());
        let png = it.next().filter(|s| !s.is_empty()).map(PathBuf::from);
        measure_inner(
            &jpg,
            png.as_deref(),
            crops.as_deref(),
            &mut output,
            pixels_only,
        )?;
        output.flush()?;
        n += 1;
        if n % 10 == 0 {
            eprintln!("chroma: {n} files");
        }
    }
    eprintln!("chroma: completed {n} files");
    Ok(())
}

/// Reproducible RGB8 synthetic sources and q50/75/85/95 4:2:0 encodings.
/// Writes every pair to a manifest, preserving dimensions and edge phase.
pub(crate) fn synthetic(args: &[String]) -> Result<()> {
    if args.len() != 1 {
        return Err("usage: imij-cmp jpegsize-synthetic NEW_DIRECTORY".into());
    }
    let dir = Path::new(&args[0]);
    fs::create_dir(dir)?;
    let mut patterns: Vec<String> = [
        "grad_h", "grad_v", "grad_d", "grad_r", "luma_h", "luma_v", "noise",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for axis in ["h", "v"] {
        for rate in [1, 2, 4, 8, 16, 32] {
            patterns.push(format!("ramp_{axis}_{rate}"));
        }
        for pair in ["rc", "by"] {
            for phase in 0..16 {
                patterns.push(format!("edge_{axis}_{pair}_{phase}"));
            }
            for width in [1, 2] {
                patterns.push(format!("line_{axis}_{pair}_{width}"));
            }
        }
    }
    for period in [2, 4, 8, 16, 32] {
        patterns.push(format!("check_{period}"));
    }
    let mut manifest = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("manifest.txt"))?;
    let mut provenance = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("sources.jsonl"))?;
    for size in [256usize, 257, 1023, 1024] {
        for pattern in &patterns {
            let parts: Vec<_> = pattern.split('_').collect();
            let pair = |tag: &str| {
                if tag == "rc" {
                    ([255, 0, 0], [0, 255, 255])
                } else {
                    ([0, 0, 255], [255, 255, 0])
                }
            };
            let mut state = 0x243f6a8885a308d3u64;
            let mut pixels = Vec::with_capacity(size * size);
            for y in 0..size {
                for x in 0..size {
                    let color = match parts[0] {
                        "edge" => {
                            let (a, b) = pair(parts[2]);
                            let phase: usize = parts[3].parse()?;
                            let boundary = (size / 2 / 16) * 16 + phase;
                            if (if parts[1] == "h" { x } else { y }) < boundary {
                                a
                            } else {
                                b
                            }
                        }
                        "line" => {
                            let (a, b) = pair(parts[2]);
                            let width: usize = parts[3].parse()?;
                            let position = if parts[1] == "h" { x } else { y };
                            if (size / 2..size / 2 + width).contains(&position) {
                                a
                            } else {
                                b
                            }
                        }
                        "check" => {
                            let period: usize = parts[1].parse()?;
                            if (x / period + y / period) % 2 == 0 {
                                [255, 0, 0]
                            } else {
                                [0, 255, 255]
                            }
                        }
                        "noise" => {
                            state ^= state << 13;
                            state ^= state >> 7;
                            state ^= state << 17;
                            let n = (state % 7) as i16 - 3;
                            [(110 + n) as u8, (140 + n) as u8, (170 + n) as u8]
                        }
                        _ => {
                            let t = match parts[0] {
                                "ramp" => {
                                    let rate: usize = parts[2].parse()?;
                                    let position = if parts[1] == "h" { x } else { y };
                                    let value = (position / rate) % 160;
                                    if value <= 80 {
                                        value as f32 / 80.0
                                    } else {
                                        (160 - value) as f32 / 80.0
                                    }
                                }
                                _ => match parts[1] {
                                    "h" => x as f32 / (size - 1) as f32,
                                    "v" => y as f32 / (size - 1) as f32,
                                    "d" => (x + y) as f32 / (2 * (size - 1)) as f32,
                                    "r" => {
                                        let center = (size - 1) as f32 * 0.5;
                                        (((x as f32 - center).powi(2)
                                            + (y as f32 - center).powi(2))
                                        .sqrt()
                                            / center)
                                            .min(1.0)
                                    }
                                    _ => unreachable!(),
                                },
                            };
                            let (luma, cb, cr) = if parts[0] == "luma" {
                                (32.0 + 192.0 * t, 136.0, 120.0)
                            } else {
                                (128.0, 88.0 + 80.0 * t, 168.0 - 80.0 * t)
                            };
                            let (r, g, b) = zenjpeg::color::ycbcr_to_rgb_f32(luma, cb, cr);
                            [
                                r.round().clamp(0.0, 255.0) as u8,
                                g.round().clamp(0.0, 255.0) as u8,
                                b.round().clamp(0.0, 255.0) as u8,
                            ]
                        }
                    };
                    pixels.push(rgb::Rgb::new(color[0], color[1], color[2]));
                }
            }
            let stem = format!("{pattern}_{size}");
            let pngpath = dir.join(format!("{stem}.png"));
            write_png(dir, &format!("{stem}.png"), pixels, size, size)?;
            let pngbytes = fs::read(&pngpath)?;
            if pngbytes.get(24..26) != Some(&[8, 2]) {
                return Err("synthetic PNG must be truecolor RGB8".into());
            }
            let pngsha = format!("{:x}", Sha256::digest(&pngbytes));
            for q in [50, 75, 85, 95] {
                let jpgpath = dir.join(format!("{stem}_q{q}_420.jpg"));
                zenenc(&[
                    jpgpath.to_string_lossy().into_owned(),
                    pngpath.to_string_lossy().into_owned(),
                    q.to_string(),
                ])?;
                writeln!(manifest, "{}\t{}", jpgpath.display(), pngpath.display())?;
                writeln!(
                    provenance,
                    "{}",
                    serde_json::json!({"pattern":pattern,"size":size,"quality":q,"sampling":"420","png_sha256":pngsha,"jpeg_sha256":format!("{:x}",Sha256::digest(fs::read(&jpgpath)?))})
                )?;
            }
            manifest.flush()?;
            provenance.flush()?;
            eprintln!("synthetic: {stem}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_catmull_rom_uses_the_declared_clamped_matrix() {
        // Exact N=8 profile bits, derived from r/2-1/4 and coalesced clamp taps.
        let expected: [[u32; 8]; 16] = [
            [
                0x3f890000, 0xbd900000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0x3f4c0000, 0x3e680000, 0xbcc00000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0x3e500000, 0x3f5e0000, 0xbd900000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0xbd900000, 0x3f5e0000, 0x3e680000, 0xbcc00000, 0x00000000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0xbcc00000, 0x3e680000, 0x3f5e0000, 0xbd900000, 0x00000000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0x00000000, 0xbd900000, 0x3f5e0000, 0x3e680000, 0xbcc00000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0x00000000, 0xbcc00000, 0x3e680000, 0x3f5e0000, 0xbd900000, 0x00000000, 0x00000000,
                0x00000000,
            ],
            [
                0x00000000, 0x00000000, 0xbd900000, 0x3f5e0000, 0x3e680000, 0xbcc00000, 0x00000000,
                0x00000000,
            ],
            [
                0x00000000, 0x00000000, 0xbcc00000, 0x3e680000, 0x3f5e0000, 0xbd900000, 0x00000000,
                0x00000000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0xbd900000, 0x3f5e0000, 0x3e680000, 0xbcc00000,
                0x00000000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0xbcc00000, 0x3e680000, 0x3f5e0000, 0xbd900000,
                0x00000000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0xbd900000, 0x3f5e0000, 0x3e680000,
                0xbcc00000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0xbcc00000, 0x3e680000, 0x3f5e0000,
                0xbd900000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0xbd900000, 0x3f5e0000,
                0x3e500000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0xbcc00000, 0x3e680000,
                0x3f4c0000,
            ],
            [
                0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0xbd900000,
                0x3f890000,
            ],
        ];
        let input = [
            core::f32::consts::PI,
            -core::f32::consts::E,
            1.0 / 3.0,
            123.456,
            -77.777,
            0.00013,
            core::f32::consts::SQRT_2,
            -0.57721,
        ];
        for r in 0..16 {
            let (i, weights) = cr_weights(r);
            for c in 0..8 {
                assert_eq!(
                    cr_tap(|j| if j == c { 1.0 } else { 0.0 }, i, weights, 8).to_bits(),
                    expected[r][c]
                );
            }
            let mut sum = 0.0f32;
            for c in 0..8 {
                sum += f32::from_bits(expected[r][c]) * input[c];
            }
            assert_eq!(cr_tap(|j| input[j], i, weights, 8).to_bits(), sum.to_bits());
        }
    }

    #[test]
    fn padded_rgb_rows_preserve_statistics_corrections_and_metrics() {
        let (w, h) = (9usize, 17usize);
        let mut a = vec![0; w * h * 3];
        let mut b = a.clone();
        for (i, v) in a.iter_mut().enumerate() {
            *v = (i * 17) as u8;
            b[i] = v.wrapping_add((i % 5) as u8);
        }
        let pack = |src: &[u8], stride: usize| {
            let mut out = vec![199u8; stride * h];
            for r in 0..h {
                out[r * stride..r * stride + w * 3]
                    .copy_from_slice(&src[r * w * 3..(r + 1) * w * 3]);
            }
            out
        };
        let aa = pack(&a, w * 3 + 11);
        let bb = pack(&b, w * 3 + 7);
        let s = diffs(&a, &b, w, h, w * 3, w * 3);
        let t = diffs(&aa, &bb, w, h, w * 3 + 11, w * 3 + 7);
        assert_eq!(s.channel_hist, t.channel_hist);
        assert_eq!(s.mean_abs, t.mean_abs);
        assert_eq!(s.worst, t.worst);
        assert_eq!(
            serde_json::to_string(&correction(&a, &b, w, h, w * 3, w * 3).unwrap()).unwrap(),
            serde_json::to_string(&correction(&aa, &bb, w, h, w * 3 + 11, w * 3 + 7).unwrap())
                .unwrap()
        );
        assert_eq!(
            quality(&a, &b, w, h, w * 3, w * 3),
            quality(&aa, &bb, w, h, w * 3 + 11, w * 3 + 7)
        );
        assert_eq!(
            imij_ll(&a, w, h, w * 3).unwrap(),
            imij_ll(&aa, w, h, w * 3 + 11).unwrap()
        );
    }

    #[test]
    fn full_pair_metric_matches_cached_reference_and_serial_execution() {
        for (w, h) in [(256usize, 257usize), (257, 259)] {
            let reference: Vec<u8> = (0..w * h * 3).map(|i| (i * 13 + i / 7) as u8).collect();
            let mut image = reference.clone();
            for y in 15..18 {
                for x in 250..255 {
                    image[(y * w + x) * 3] = image[(y * w + x) * 3].wrapping_add(7);
                }
            }
            let r = fast_ssim2::PixelSlice::new(
                &reference,
                w as u32,
                h as u32,
                w * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )
            .unwrap();
            let d = fast_ssim2::PixelSlice::new(
                &image,
                w as u32,
                h as u32,
                w * 3,
                fast_ssim2::PixelDescriptor::RGB8_SRGB,
            )
            .unwrap();
            let cached = fast_ssim2::Ssimulacra2Reference::new(&r)
                .unwrap()
                .compare(&d)
                .unwrap();
            let full = fast_ssim2::compute_ssimulacra2(&r, &d).unwrap();
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .unwrap();
            let serial = pool
                .install(|| fast_ssim2::compute_ssimulacra2(&r, &d))
                .unwrap();
            assert_eq!(full.to_bits(), serial.to_bits());
            assert_eq!(format!("{cached:.4}"), format!("{full:.4}"));
            eprintln!(
                "metric parity {w}x{h}: cached={cached:.12} full={full:.12} serial={serial:.12}"
            );
        }
    }

    #[test]
    fn strip_metric_matches_full_metric_on_odd_multistrip_images() {
        let (w, h) = (257usize, 259usize);
        let mut state = 0x12345678u32;
        let reference: Vec<u8> = (0..w * h * 3)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let mut image = reference.clone();
        for y in 250..258 {
            for x in 15..18 {
                image[(y * w + x) * 3] = image[(y * w + x) * 3].wrapping_add(7);
            }
        }
        let full = butteraugli::ButteraugliReference::new(
            &reference,
            w,
            h,
            butteraugli::ButteraugliParams::default(),
        )
        .unwrap()
        .compare(&image)
        .unwrap()
        .score;
        let rgb = |p: &[u8]| -> Vec<rgb::Rgb<u8>> {
            p.chunks_exact(3)
                .map(|v| rgb::Rgb::new(v[0], v[1], v[2]))
                .collect()
        };
        let r = rgb(&reference);
        let i = rgb(&image);
        let strip = butteraugli::butteraugli_strip(
            imgref::ImgRef::new(&r, w, h),
            imgref::ImgRef::new(&i, w, h),
            &butteraugli::ButteraugliParams::default(),
            256,
        )
        .unwrap()
        .score;
        assert_eq!(full.to_bits(), strip.to_bits());
    }

    #[test]
    fn modular_correction_covers_extreme_signed_deltas() {
        let a = [0, 255, 128, 255, 0, 127, 13, 17, 19, 20, 30, 40];
        let b = [255, 0, 128, 0, 255, 127, 13, 17, 19, 20, 30, 40];
        let c = correction(&a, &b, 2, 2, 6, 6).unwrap();
        assert_eq!((c.n_rows, c.n_cols), (1, 2));
        assert!(c.hist.contains("-255:2"));
        assert!(c.hist.contains("255:2"));
        assert!(c.d_rows > 0 && c.d_cols > 0 && c.p_rows > 0 && c.p_cols > 0);
        let zero = correction(&a, &a, 2, 2, 6, 6).unwrap();
        assert_eq!(
            (zero.d_rows, zero.d_cols, zero.p_rows, zero.p_cols),
            (0, 0, 0, 0)
        );
    }
    #[test]
    fn visible_edges_exclude_coded_padding_and_accept_stride() {
        let coded = [10.0, 20.0, 999.0, 30.0, 40.0, 999.0, 999.0, 999.0, 999.0];
        let plane = visible(&coded, 3, 2, 2);
        assert_eq!(plane, [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(up_triangle(&plane, 2, 2, 2, 2).last(), Some(&40.0));
        assert_eq!(up_plane_cr(&plane, 2, 2, 2).len(), 16);
        assert_eq!(up_plane_cr(&coded, 2, 2, 3), up_plane_cr(&plane, 2, 2, 2));
        assert_eq!(
            up_triangle(&coded, 2, 2, 2, 3),
            up_triangle(&plane, 2, 2, 2, 2)
        );
        assert_eq!(
            up_integer(&[10, 20, 999, 30, 40, 999], 2, 2, 3, 3, 3),
            up_integer(&[10, 20, 30, 40], 2, 2, 2, 3, 3)
        );
        let s = diffs(&[0, 1, 2, 3, 7, 255], &[0; 6], 2, 1, 6, 6);
        assert_eq!(
            s.channel_hist,
            [[1, 0, 0, 1, 0, 0], [0, 1, 0, 0, 1, 0], [0, 0, 1, 0, 0, 1]]
        );
        assert_eq!(s.gt2, 0.5);
    }
}

/// Fresh comparison of current decoder output with the independent oracle.
pub(crate) fn parity(args: &[String]) -> Result<()> {
    if args.len() != 2 {
        return Err("usage: imij-cmp jpegsize-parity OUT.jsonl MANIFEST".into());
    }
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[0])?;
    let mut n = 0;
    for line in fs::read_to_string(&args[1])?
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let path = Path::new(line.split('\t').next().unwrap());
        let data = fs::read(path)?;
        let h = jpeg_head(&data)?;
        let (w, hh) = (h.width as usize, h.height as usize);
        let target = exact_target(path, w * hh * 3)?;
        let pixels = decode_rgb(&data)?;
        if pixels.len() != target.len() {
            return Err("parity geometry mismatch".into());
        }
        let s = diffs(&pixels, &target, w, hh, w * 3, w * 3);
        let count = pixels
            .chunks_exact(3)
            .zip(target.chunks_exact(3))
            .filter(|(a, b)| a != b)
            .count();
        writeln!(
            out,
            "{}",
            serde_json::json!({"sha256":format!("{:x}",Sha256::digest(&data)),"width":w,"height":hh,"components":h.ncomp,"progressive":h.sof == 194,"restart":h.restart,"max_abs":s.max_abs,"differing_pixels":count,"channel_hist":s.channel_hist})
        )?;
        out.flush()?;
        n += 1;
    }
    if n < 20 {
        return Err("canonical parity set requires at least 20 files".into());
    }
    Ok(())
}

/// Export crops from retained RGB8 artifacts. Selection is a JSON array with
/// input_sha256, arm and case; no decoder or metric is rerun.
pub(crate) fn crops_from_pixels(args: &[String]) -> Result<()> {
    if args.len() != 3 {
        return Err(
            "usage: imij-cmp jpegsize-crops PIXEL_ROOT SELECTION.json OUTPUT_DIRECTORY".into(),
        );
    }
    let root = Path::new(&args[0]);
    let out = Path::new(&args[2]);
    let selection: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut results = Vec::new();
    for selected in selection {
        let sha = selected["input_sha256"]
            .as_str()
            .ok_or("missing input sha")?;
        let arm = selected["arm"].as_str().ok_or("missing arm")?;
        let case = selected["case"].as_str().ok_or("missing crop case")?;
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) || !ARMS.contains(&arm) {
            return Err("invalid crop identity".into());
        }
        if !["worst_edge", "gradient", "sharp_edge"].contains(&case) {
            return Err("unknown crop case".into());
        }
        let paths: Vec<_> = fs::read_dir(root.join("indexes"))?
            .filter_map(|v| v.ok())
            .map(|v| v.path())
            .filter(|v| v.file_name().unwrap().to_string_lossy().starts_with(sha))
            .collect();
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|p| {
                fs::read(p)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .is_some_and(|index| index["interpolation_profile"] == "coalesced-clamp")
            })
            .collect();
        if paths.len() != 1 {
            return Err(format!("expected one pixel index for {sha}, got {}", paths.len()).into());
        }
        let index: serde_json::Value = serde_json::from_slice(&fs::read(&paths[0])?)?;
        let (w, h) = (
            index["width"].as_u64().ok_or("width")? as usize,
            index["height"].as_u64().ok_or("height")? as usize,
        );
        let read = |name: &str| -> Result<Vec<u8>> {
            if Path::new(name).file_name().and_then(|p| p.to_str()) != Some(name) {
                return Err("invalid pixel filename".into());
            }
            let bytes = fs::read(root.join(name))?;
            if bytes.len() != w * h * 3
                || !name.starts_with(&format!("{:x}", Sha256::digest(&bytes)))
            {
                return Err("pixel artifact hash/geometry differs".into());
            }
            Ok(bytes)
        };
        let image = read(index["arms"][arm].as_str().ok_or("missing arm image")?)?;
        let target = read(index["target_i"].as_str().ok_or("missing target")?)?;
        let b = read(
            index["arms"]["b"]
                .as_str()
                .unwrap_or(index["target_i"].as_str().unwrap()),
        )?;
        let mut peak = (0, 0, 0u8);
        for y in 0..h {
            for x in 0..w {
                if y % 16 != 0 && y % 16 != 15 && x % 16 != 0 && x % 16 != 15 {
                    continue;
                }
                let at = (y * w + x) * 3;
                let d = (0..3)
                    .map(|c| image[at + c].abs_diff(target[at + c]))
                    .max()
                    .unwrap();
                if d > peak.2 {
                    peak = (x, y, d);
                }
            }
        }
        let mut files = Vec::new();
        for (tag, pixels) in [
            ("arm", image.as_slice()),
            ("b", b.as_slice()),
            ("i", target.as_slice()),
        ] {
            let (buf, cw, ch) = crop(pixels, w, h, w * 3, peak.0, peak.1, 64);
            let name = format!("{case}_{arm}_{sha}_{tag}.png");
            write_png(out, &name, buf, cw, ch)?;
            files.push(name);
        }
        results.push(serde_json::json!({"input_sha256":sha,"arm":arm,"case":case,"grid_edge_worst":[peak.0,peak.1,peak.2],"files":files}));
    }
    fs::write(out.join("crops.json"), serde_json::to_vec_pretty(&results)?)?;
    Ok(())
}
