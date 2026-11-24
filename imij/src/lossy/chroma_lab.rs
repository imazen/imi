//! Lab-only coefficient bridge for the mapped 4x4 chroma experiment.
//! No normative production decoder changes. RGB8 sRGB only, no alpha/AQ/groups.
use super::*;
/// Existing production zigzag for reversible packing diagnostics.
pub const ZIGZAG8: [u8; 64] = ZIGZAG;

/// Lab pixel and planned scratch budgets, separate from production limits.
pub const MAX_PADDED_PIXELS: usize = 4_194_304;
/// Conservative charge for full-image planes/coefficient copies; not an allocator
/// guarantee. The independent pixel cap bounds allocations even on 32-bit hosts.
pub const MAX_ESTIMATED_WORKING_BYTES: usize = 512 * 1024 * 1024;
const WORKING_BYTES_PER_PADDED_PIXEL: usize = 128;

/// Checked original dimensions, padded dimensions, and padded pixel count.
/// Validate before any pixel allocation or nested coefficient parsing.
pub fn checked_geometry(
    width: usize,
    height: usize,
) -> Result<(u32, u32, usize, usize, usize), Error> {
    let bad = || Error::InvalidInput("chroma lab geometry/working-set cap");
    if width == 0 || height == 0 {
        return Err(bad());
    }
    let w = u32::try_from(width).map_err(|_| bad())?;
    let h = u32::try_from(height).map_err(|_| bad())?;
    let pad = |n: usize| {
        n.checked_add(7)
            .map(|v| v / 8)
            .and_then(|v| v.checked_mul(8))
            .ok_or_else(bad)
    };
    let pw = pad(width)?;
    let ph = pad(height)?;
    let pixels = pw.checked_mul(ph).ok_or_else(bad)?;
    let working = pixels
        .checked_mul(WORKING_BYTES_PER_PADDED_PIXEL)
        .ok_or_else(bad)?;
    if pixels > MAX_PADDED_PIXELS || working > MAX_ESTIMATED_WORKING_BYTES {
        return Err(bad());
    }
    Ok((w, h, pw, ph, pixels))
}

fn checked_file_geometry(file: &[u8]) -> Result<(), Error> {
    // Header-only parsing avoids nested DC/AC allocations on oversized files.
    let info = crate::header(file)?.info;
    checked_geometry(info.width as usize, info.height as usize)?;
    Ok(())
}

/// Shared production parameters, including quality clamping (q0 == q1).
pub fn parameters(quality: f32, color: LossyColor) -> ([f32; 3], [[f32; 64]; 3]) {
    let matrix = if color == LossyColor::Xyb { 2 } else { 0 };
    (
        color.default_scales(),
        QParams::from_quality(quality, color, matrix).steps(),
    )
}

/// Production color conversion, right/bottom replication to whole 8x8 blocks.
/// Stride is bytes; only RGB8 sRGB is accepted, alpha is absent.
pub fn planes(
    rgb: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    color: LossyColor,
) -> Result<[Vec<f32>; 3], Error> {
    let (w, h, pw, ph, pixels) = checked_geometry(width, height)?;
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("RGB8 layout"))?;
    if width == 0 || height == 0 || rgb.len() < extent {
        return Err(Error::InvalidInput("RGB8 extent"));
    }
    let mut p: [Vec<f32>; 3] = core::array::from_fn(|_| vec![0.0; pixels]);
    for y in 0..ph {
        for x in 0..pw {
            for c in 0..3 {
                let v = rgb[y.min(height - 1) * stride + x.min(width - 1) * 3 + c];
                p[c][y * pw + x] = if color == LossyColor::Xyb {
                    color::SRGB8_TO_LINEAR[v as usize] as f32
                } else {
                    v as f32
                };
            }
        }
    }
    let [a, b, c] = &mut p;
    enc_simd::color_rows(
        color == LossyColor::Xyb,
        color.default_scales(),
        &color::OPSIN_F32,
        color::OPSIN_BIAS,
        color::CBRT_BIAS,
        a,
        b,
        c,
    );
    Ok(p)
}

/// Canonical 8x8 inverse, raster quantized coefficients and production steps.
pub fn inverse8(q: &[i32], steps: &[f32; 64]) -> [f32; 64] {
    let dq = dct::dequant_factors(steps);
    let mut x = core::array::from_fn(|i| q[i] as f32 * dq[i]);
    dct::idct8x8(&mut x);
    x
}

/// Canonical inverse color and sRGB8 conversion.
pub fn inverse_color(color: LossyColor, p: [f32; 3]) -> [u8; 3] {
    color::inverse_px(color, color.default_scales().map(|v| 1.0 / v), p)
}

/// Entropy-decode a production RGB8 run-level file to exact raster coefficients.
pub fn unpack(file: &[u8]) -> Result<Vec<i32>, Error> {
    checked_file_geometry(file)?;
    let p = parse_lossy(file)?;
    if p.layouts != [LossyChannelLayout::default(); 3]
        || p.info.channels != Channels::Rgb
        || p.info.sample != SampleType::U8
        || p.aq
        || p.skip
        || p.gcols != 0
        || p.tables.mode != LossyEntropy::RunLevel
    {
        return Err(Error::Unsupported(
            "chroma lab requires ungrouped RGB8 run-level",
        ));
    }
    let dcp = p.dc.as_ref().unwrap();
    let om = crate::v1::OutMap::new(&dcp.info, &dcp.layout);
    let mut dc = Vec::new();
    for u in 0..dcp.stripes.len() {
        let rows = dcp.hs.min(p.g.bh - u * dcp.hs);
        let stride = p.g.bw * dcp.bpp();
        let mut px = vec![0; rows * stride];
        crate::v1::decode_unit_into_checked(dcp, u, &om, &mut px, stride, &|| Ok(()))?;
        dc_values(dcp.info.sample, false, false, &px, &mut dc, &mut Vec::new())?;
    }
    let mut sc = Scratch::default();
    let mut q = Vec::new();
    for s in 0..p.g.stripes() {
        let (by0, by1) = p.g.rows(s);
        decode_stripe_coefs(&p, &p.g, s, s, &dc[by0 * p.g.bw * 3..], false, &mut sc)?;
        q.extend_from_slice(&sc.q[..(by1 - by0) * p.g.bw * 192]);
        sc.q.fill(0);
    }
    Ok(q)
}

/// Re-serialize mapped coefficients through production run-level AC and lossless
/// DC coding, complete native inner container. Same q/settings as production444.
/// 4x4 slots live at raster (v*8+u), others zero; its inverse is lab-only.
pub fn pack(
    q: &[i32],
    width: u32,
    height: u32,
    quality: f32,
    color: LossyColor,
) -> Result<Vec<u8>, Error> {
    checked_geometry(width as usize, height as usize)?;
    let g = Geom::new(width, height, 1);
    if width == 0
        || height == 0
        || q.len() != g.bw * g.bh * 192
        || q.iter().any(|&v| v.unsigned_abs() > 4194303)
    {
        return Err(Error::InvalidInput("coefficient extent/range"));
    }
    let cfg = LossyConfig {
        quality,
        color,
        threads: 6,
        entropy: LossyEntropy::RunLevel,
        column_groups: Some(0),
        ..Default::default()
    };
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let stripes: Vec<_> = (0..g.bh)
        .map(|by| {
            let q = q[by * g.bw * 192..(by + 1) * g.bw * 192].to_vec();
            let nz = q
                .as_chunks::<64>()
                .0
                .iter()
                .map(|c| {
                    ZIGZAG
                        .iter()
                        .enumerate()
                        .fold(0u64, |m, (k, &i)| m | ((c[i as usize] != 0) as u64) << k)
                })
                .collect();
            StripeCoefs {
                by0: by,
                nby: 1,
                bw: g.bw,
                q,
                nz,
                aq: Vec::new(),
                modes: Vec::new(),

                splits: Vec::new(),
            }
        })
        .collect();
    let fits16 = q
        .as_chunks::<64>()
        .0
        .iter()
        .all(|b| (-32768..32768).contains(&b[0]));
    let mut dc = Vec::new();
    for b in q.as_chunks::<64>().0.iter() {
        if fits16 {
            dc.extend_from_slice(&((b[0] + 32768) as u16).to_ne_bytes());
        } else {
            dc.extend_from_slice(&b[0].to_ne_bytes());
        }
    }
    let dcs = if fits16 { 2 } else { 4 };
    let dc_info = ImageInfo::new(
        g.bw as u32,
        g.bh as u32,
        Channels::Rgb,
        if fits16 {
            SampleType::U16
        } else {
            SampleType::I32
        },
    );
    let dc_cfg = crate::EncodeConfig {
        effort: cfg.effort,
        stripe_rows: Some(8),
        threads: 6,
        column_groups: Some(0),
        ..Default::default()
    };
    let (dc_head, dc_stripes, _) = crate::v1::encode_parts(&dc, g.bw * 3 * dcs, &dc_info, &dc_cfg)?;
    let (tb, ac, _, _) = encode_ans_stripes(&stripes, &cfg, effort, &how_far::NoPulse)?;
    let qp = QParams::from_quality(quality, color, if color == LossyColor::Xyb { 2 } else { 0 });
    use crate::parse::records::{R_AC, R_DC, put_record};
    // SPEC 2.8: one block row per stripe, no column groups, the default
    // parameters with ordinary restart states, the tables, the 32-bit DC
    // record when needed, the DC head, then the framed records (no hint).
    let mut data = vec![1];
    put_varint(&mut data, 0);
    super::envelope::write(
        &mut data,
        color,
        super::InputKind::U8,
        &super::envelope::Params {
            scale: color.default_scales(),
            qp,
            entropy: LossyEntropy::RunLevel,
            compact: false,
            seed: crate::seeds::Selection::NONE,
            records: !fits16,
        },
    );
    put_varint(&mut data, tb.len() as u64);
    data.extend_from_slice(&tb);
    if !fits16 {
        let mut records = Vec::new();
        put_record(&mut records, super::H_DC32, &[]);
        put_varint(&mut data, records.len() as u64);
        data.extend_from_slice(&records);
    }
    put_varint(&mut data, dc_head.len() as u64);
    data.extend_from_slice(&dc_head);
    for (s, ac) in ac.iter().enumerate() {
        if s % 8 == 0 {
            put_record(&mut data, R_DC, &dc_stripes[s / 8]);
        }
        put_record(&mut data, R_AC, ac);
    }
    let mut out = Vec::new();
    let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
    // An i32 DC image declares `samples_32` (SPEC 0.1).
    let features = if fits16 {
        crate::parse::header::Features::EMPTY
    } else {
        crate::parse::header::Features::EMPTY.with(crate::parse::header::Feature::Samples32)
    };
    crate::container::write_head_features(
        &mut out,
        &info,
        &Metadata::default(),
        crate::Coding::Lossy,
        Some(color),
        cfg.decode_speed(),
        features,
    )?;
    crate::container::put_data(&mut out, &data);
    Ok(out)
}

/// Actual byte accounting: container, fixed parameters/record framing, AC
/// tables, nested DC head+records, AC lane records. Their sum is complete bytes.
pub fn byte_parts(file: &[u8]) -> Result<[usize; 5], Error> {
    checked_file_geometry(file)?;
    let (info, _, body) = crate::v1::parse_header(file)?;
    let p = parse_lossy(file)?;
    // SPEC 2.8: stripe rows, column groups, the parameters, the tables,
    // the head records, then the DC head.
    let mut r = ByteReader::new(body);
    r.u8()?;
    r.var_usize()?;
    let kind = super::InputKind::of(&info).ok_or(Error::Corrupt("pixel format"))?;
    let params = super::envelope::read(
        &mut r,
        p.color,
        kind,
        crate::parse::header::Features::ALL,
        true,
    )?;
    let tables = r.var_usize()?;
    r.bytes(tables)?;
    if params.records {
        let n = r.var_usize()?;
        r.bytes(n)?;
    }
    let dc_head = r.var_usize()?;
    let dc = dc_head
        + p.dc
            .as_ref()
            .unwrap()
            .stripes
            .iter()
            .map(|s| s.len())
            .sum::<usize>();
    let ac = p.stripes.iter().map(|s| s.len()).sum::<usize>();
    Ok([
        file.len() - body.len(),
        body.len() - tables - dc - ac,
        tables,
        dc,
        ac,
    ])
}

/// Canonical encoder 8x8 transform, full-resolution plane raster with stride 8.
pub fn forward8(pixels: &[f32; 64]) -> [f32; 64] {
    let mut out = [0.0; 64];
    enc_simd::fdct_row(pixels, 8, 1, &dct::dct_matrix(), &mut out);
    out
}

/// Canonical encoder quantization of one channel, with no adaptive multiplier.
pub fn quantize8(coefs: &[f32; 64], steps: &[f32; 64], round: f32) -> [i32; 64] {
    let mut out = [0; 192];
    enc_simd::quant_row(
        coefs,
        1,
        0,
        &steps.map(|v| 1.0 / v),
        &[],
        round.clamp(0.0, 0.5),
        &mut out,
        &mut [0; 3],
    );
    out[..64].try_into().unwrap()
}

/// Actual color and orthonormal quantizer steps from a bounded RGB8 native
/// run-level image. Restoration experiments must not duplicate these in an
/// outer wrapper where they could disagree with the coded coefficients.
pub fn unpack_parameters(file: &[u8]) -> Result<(LossyColor, [[f32; 64]; 3]), Error> {
    checked_file_geometry(file)?;
    let p = parse_lossy(file)?;
    if p.info.channels != Channels::Rgb
        || p.info.sample != SampleType::U8
        || p.layouts != [LossyChannelLayout::default(); 3]
        || p.aq
        || p.skip
        || p.gcols != 0
        || p.tables.mode != LossyEntropy::RunLevel
        || p.inv_scale != p.color.default_scales().map(|v| 1.0 / v)
    {
        return Err(Error::Unsupported(
            "restoration lab requires default-scale RGB8 run-level",
        ));
    }
    Ok((p.color, p.layout_steps.map(|steps| steps[0])))
}

#[cfg(all(test, feature = "encode"))]
mod geometry_tests {
    use super::*;
    #[test]
    fn oversized_native_header_rejected_before_nested_parse() {
        let info = ImageInfo::new(16384, 16384, Channels::Rgb, SampleType::U8);
        let mut file = Vec::new();
        crate::container::write_head(
            &mut file,
            &info,
            &Metadata::default(),
            crate::Coding::Lossy,
            Some(LossyColor::Xyb),
            crate::DecodeSpeed::Compact,
        )
        .unwrap();
        crate::container::put_data(&mut file, &[1]);
        assert!(matches!(
            unpack(&file),
            Err(Error::InvalidInput("chroma lab geometry/working-set cap"))
        ));
        assert!(matches!(
            byte_parts(&file),
            Err(Error::InvalidInput("chroma lab geometry/working-set cap"))
        ));
    }
}
