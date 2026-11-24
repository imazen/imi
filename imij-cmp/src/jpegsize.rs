//! jpegsize: can IMI hold a JPEG's own quantized DCT coefficients in fewer
//! bytes than the JPEG (or than JPEG XL's lossless JPEG transcode)? Lab
//! study — one TSV row per input file.
//!
//! ```text
//! imij-cmp jpegsize OUT.tsv FILE.jpg...
//! imij-cmp jpegsize-scan OUT.tsv FILE.jpg...   (classification only)
//! imij-cmp jpegsize-ppm FILE.jpg OUT.ppm     (zenjpeg's libjpeg-exact decode)
//! ```
//!
//! Arms per file (APPn/COM metadata stripped in every arm; `meta_bytes`
//! holds what stripping removed so totals can add it back): the original
//! bytes, the same coefficients re-packed with optimized Huffman tables
//! sequential and progressive (zenjpeg `restructure`), JPEG XL's lossless
//! JPEG transcode (`encode_jpeg_transcode`; sources it cannot take go
//! through the baseline repack first, `jxl_via`), and IMI holding the same
//! quantized coefficients (`imij::lab::encode_lossy_quantized` at the
//! JPEG's own tables): for 4:4:4 one three-plane file plus the three
//! single-plane proxy files, for 4:2:0 and grayscale the proxy files only
//! (Y, Cb, Cr at their own coded dimensions), each in run-level and
//! split-Huffman entropy. `oracle` decodes every written file's
//! coefficients back (`lossy_quantized`) and compares them to the JPEG's;
//! for 4:4:4 `px_oracle` also compares decoded pixels to an f32-IDCT
//! reference through the file's signalled transform.

use crate::coefjpeg::ZIGZAG;
use crate::rd::Result;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// The signalled decode transform: JPEG's centered YCbCr, offset +128.
pub(crate) const YCBCR_CENTERED: [[f32; 4]; 3] = [
    [1.0, 0.0, 1.402, 128.0],
    [1.0, -0.344136, -0.714136, 128.0],
    [1.0, 1.772, 0.0, 128.0],
];

const M_SOI: u8 = 0xD8;
const M_EOI: u8 = 0xD9;
const M_SOS: u8 = 0xDA;
const M_DQT: u8 = 0xDB;
const M_DRI: u8 = 0xDD;
const M_DAC: u8 = 0xCC;
const M_COM: u8 = 0xFE;
const M_TEM: u8 = 0x01;
const M_APP0: u8 = 0xE0;
const M_APP1: u8 = 0xE1;
const M_APP14: u8 = 0xEE;
const M_APP15: u8 = 0xEF;

/// Header scan of one JPEG: frame fields, marker tallies, and the same file
/// without APPn/COM segments (every measured arm codes the stripped form).
pub(crate) struct Head {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) precision: u8,
    pub(crate) ncomp: usize,
    /// (id, h, v, tq) per component in SOF order.
    pub(crate) comps: Vec<(u8, u8, u8, u8)>,
    /// First SOF marker id (0xC0 baseline, 0xC2 progressive, ...).
    pub(crate) sof: u8,
    pub(crate) restart: Option<u16>,
    pub(crate) arithmetic: bool,
    pub(crate) dqt16: bool,
    pub(crate) scans: usize,
    pub(crate) adobe_transform: Option<u8>,
    pub(crate) exif_ycbcr: Option<u16>,
    /// Bytes removed by stripping (APPn + COM).
    pub(crate) meta_bytes: usize,
    /// Bytes after the first EOI (a second JPEG, a thumbnail, garbage).
    pub(crate) tail_bytes: usize,
    pub(crate) stripped: Vec<u8>,
}

pub(crate) fn jpeg_head(data: &[u8]) -> Result<Head> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != M_SOI {
        return Err("not a JPEG (no SOI)".into());
    }
    let mut h = Head {
        width: 0,
        height: 0,
        precision: 0,
        ncomp: 0,
        comps: Vec::new(),
        sof: 0,
        restart: None,
        arithmetic: false,
        dqt16: false,
        scans: 0,
        adobe_transform: None,
        exif_ycbcr: None,
        meta_bytes: 0,
        tail_bytes: 0,
        stripped: Vec::with_capacity(data.len()),
    };
    h.stripped.extend_from_slice(&data[..2]);
    let mut pos = 2usize;
    // Marker layer: a segment is marker + u16 length + payload (the length
    // counts itself). An SOS's entropy data runs to the next real marker
    // (FF followed by a byte other than 00 or a RST id).
    loop {
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("truncated JPEG (no EOI)".into());
        }
        let m = data[pos];
        pos += 1;
        if m == M_EOI {
            h.stripped.extend_from_slice(&[0xFF, M_EOI]);
            h.tail_bytes = data.len() - pos;
            return Ok(h);
        }
        // Standalone markers (no length): SOI, TEM, restarts.
        if m == M_SOI || m == M_TEM || (0xD0..=0xD7).contains(&m) {
            h.stripped.extend_from_slice(&[0xFF, m]);
            continue;
        }
        if pos + 2 > data.len() {
            return Err("truncated JPEG (marker length)".into());
        }
        let len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if len < 2 || pos + len > data.len() {
            return Err("truncated JPEG (segment)".into());
        }
        let seg = &data[pos - 2..pos + len];
        let body = &data[pos + 2..pos + len];
        if (M_APP0..=M_APP15).contains(&m) || m == M_COM {
            h.meta_bytes += seg.len();
            if m == M_APP1 {
                if let Some(position) = exif_ycbcr_position(body) {
                    h.exif_ycbcr = Some(position);
                }
            }
            if m == M_APP14 && body.len() >= 12 && &body[..5] == b"Adobe" {
                h.adobe_transform = Some(body[11]);
            }
        } else {
            h.stripped.extend_from_slice(seg);
        }
        match m {
            M_DQT => {
                // Each table's precision nybble: 1 = 16-bit entries.
                let mut p = 2usize;
                while p + 1 < len && pos + p < data.len() {
                    let pq = data[pos + p] >> 4;
                    if pq == 1 {
                        h.dqt16 = true;
                    }
                    p += 1 + if pq == 1 { 128 } else { 64 };
                }
            }
            M_DRI => {
                if body.len() >= 2 && h.restart.is_none() {
                    h.restart = Some(u16::from_be_bytes([body[0], body[1]]));
                }
            }
            M_DAC => h.arithmetic = true,
            M_SOS => {
                h.scans += 1;
                let mut p = pos + len;
                while p + 1 < data.len() {
                    if data[p] == 0xFF
                        && data[p + 1] != 0x00
                        && !(0xD0..=0xD7).contains(&data[p + 1])
                    {
                        break;
                    }
                    p += 1;
                }
                h.stripped.extend_from_slice(&data[pos + len..p]);
                pos = p;
                continue;
            }
            0xC0..=0xCF if m != 0xC4 && m != 0xC8 && m != M_DAC => {
                if h.sof == 0 {
                    h.sof = m;
                    h.arithmetic |= matches!(m, 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF);
                    if body.len() >= 6 {
                        h.precision = body[0];
                        h.height = u16::from_be_bytes([body[1], body[2]]) as u32;
                        h.width = u16::from_be_bytes([body[3], body[4]]) as u32;
                        h.ncomp = body[5] as usize;
                        for c in 0..h.ncomp {
                            let o = 6 + c * 3;
                            if o + 3 <= body.len() {
                                h.comps.push((
                                    body[o],
                                    body[o + 1] >> 4,
                                    body[o + 1] & 15,
                                    body[o + 2],
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        pos += len;
    }
}

/// EXIF tag 0x0213 (YCbCrPositioning) of an APP1 `Exif\0\0` body, if any.
fn exif_ycbcr_position(body: &[u8]) -> Option<u16> {
    let t = body.strip_prefix(b"Exif\0\0")?;
    if t.len() < 8 {
        return None;
    }
    let be = match &t[..2] {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    let u16v = |o: usize| -> Option<u16> {
        let b = t.get(o..o + 2)?;
        Some(if be {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        })
    };
    let u32v = |o: usize| -> Option<u32> {
        let b = t.get(o..o + 4)?;
        Some(if be {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    if u16v(2)? != 42 {
        return None;
    }
    let ifd = u32v(4)? as usize;
    let n = u16v(ifd)? as usize;
    for i in 0..n.min(512) {
        let e = ifd + 2 + i * 12;
        if e + 12 > t.len() {
            break;
        }
        if u16v(e)? == 0x0213 && u16v(e + 2)? == 3 {
            // SHORT, count 1: the value sits in the field's first bytes.
            return u16v(e + 8);
        }
    }
    None
}

pub(crate) fn sampling(h: &Head) -> String {
    if h.ncomp == 1 {
        return "gray".into();
    }
    if h.ncomp == 3 {
        // Adobe transform 0 and explicit R/G/B component identifiers mean
        // RGB coefficients, to which the centered YCbCr matrix does not apply.
        if h.adobe_transform == Some(0) || h.comps.iter().map(|c| c.0).eq([b'R', b'G', b'B']) {
            return "rgb".into();
        }
        let hv: Vec<(u8, u8)> = h.comps.iter().map(|c| (c.1, c.2)).collect();
        return match hv.as_slice() {
            [(1, 1), (1, 1), (1, 1)] => "444".into(),
            [(2, 2), (1, 1), (1, 1)] => "420".into(),
            [(2, 1), (1, 1), (1, 1)] => "422".into(),
            [(1, 2), (1, 1), (1, 1)] => "440".into(),
            [(4, 1), (1, 1), (1, 1)] => "411".into(),
            _ => format!(
                "s{}",
                hv.iter()
                    .fold(String::new(), |s, v| s + &format!("{}{}", v.0, v.1))
            ),
        };
    }
    if h.ncomp == 4 {
        return match h.adobe_transform {
            Some(2) => "ycck".into(),
            _ => "cmyk".into(),
        };
    }
    format!("n{}", h.ncomp)
}

/// One component's blocks (zigzag `coeffs`, block-row-major) as an IMI
/// raster-order plane fragment: `out[(block * 3 + ch) * 64 + raster]`.
fn scatter_comp(q: &mut [i32], ch: usize, coeffs: &[i16]) {
    for (b, src) in coeffs.chunks_exact(64).enumerate() {
        let dst = &mut q[(b * 3 + ch) * 64..][..64];
        for (k, &v) in src.iter().enumerate() {
            dst[ZIGZAG[k]] = v as i32;
        }
    }
}

/// The textbook separable f32 IDCT (JPEG's convention: a DC-only block
/// reconstructs at `input[0] / 8`), the pixel oracle's independent
/// dequantized-coefficients-to-samples path.
pub(crate) fn idct8x8(b: &[f32; 64]) -> [f32; 64] {
    const C: [f32; 8] = [
        core::f32::consts::FRAC_1_SQRT_2,
        1.0,
        1.0,
        1.0,
        1.0,
        1.0,
        1.0,
        1.0,
    ];
    let cos: [[f32; 8]; 8] = core::array::from_fn(|u| {
        core::array::from_fn(|x| (((2 * x + 1) * u) as f32 * core::f32::consts::PI / 16.0).cos())
    });
    let mut tmp = [0f32; 64];
    for v in 0..8 {
        for x in 0..8 {
            let mut s = 0.0f32;
            for u in 0..8 {
                s += C[u] * b[v * 8 + u] * cos[u][x];
            }
            tmp[v * 8 + x] = s;
        }
    }
    let mut out = [0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0f32;
            for v in 0..8 {
                s += C[v] * tmp[v * 8 + x] * cos[v][y];
            }
            out[y * 8 + x] = s * 0.25;
        }
    }
    out
}

/// The ImiEncoderConfig an imported JPEG codes at: the JPEG's own tables as the
/// explicit matrix (natural order), scale 1, unit DC steps, and every
/// coefficient-modifying stage off. `transform` signals the centered YCbCr
/// decode matrix. Gray proxy files put their component in channel zero;
/// the same matrix adds JPEG's +128 level shift to that channel's output.
fn coef_cfg(
    entropy: imij::LossyEntropy,
    tables: [[f32; 64]; 3],
    transform: bool,
) -> imij::ImiEncoderConfig {
    let mut c = imij::ImiEncoderConfig::lossy(50.0);
    c = c.tune_lossy(|t| {
        t.channel_mul = Some([1.0, 1.0, 1.0]);
    });
    c = c.tune_lossy(|t| {
        t.entropy = entropy;
    });
    c = c.with_decode_speed(None);
    c = c.with_column_groups(Some(0));
    c = c.tune_lossy(|t| {
        t.exact_blocks = Some(0.0);
    });
    let mut lab = imij::LossyLabSwitches::defaults();
    lab.dc_step = Some([64, 64]);
    lab.explicit_matrix = Some(Box::new(tables));
    lab.color_transform = transform.then_some(imij::ColorTransform::Explicit(YCBCR_CENTERED));
    // The decode-time filter would move the pixel oracle, not coefficients.
    lab.dering = Some(false);
    c = c.tune_lossy(|t| {
        t.lab.switches = lab;
    });
    c
}

/// Encode `q` as one IMI file and check its coefficients decode back
/// exactly. Returns (file bytes, oracle string).
fn coef_file(
    w: u32,
    h: u32,
    channels: imij::Channels,
    cfg: &imij::ImiEncoderConfig,
    q: &[i32],
) -> Result<(Vec<u8>, String)> {
    let info = imij::ImageInfo::new(w, h, channels, imij::SampleType::U8);
    let file = imij::lab::encode_lossy_quantized(&info, &imij::Metadata::default(), cfg, q)
        .map_err(|e| format!("imij coef encode: {e}"))?;
    let (bw, bh, back, _, _) =
        imij::lab::lossy_quantized(&file).map_err(|e| format!("imij coef decode: {e}"))?;
    if bw * bh * 192 != q.len() || back.len() != q.len() {
        return Ok((file, format!("bad-grid {bw}x{bh}")));
    }
    let (mut diff, mut first) = (0usize, 0usize);
    for (i, (&a, &b)) in q.iter().zip(&back).enumerate() {
        if a != b {
            if diff == 0 {
                first = i;
            }
            diff += 1;
        }
    }
    Ok(if diff == 0 {
        (file, "ok".into())
    } else {
        (file, format!("mismatch n={diff} @{first}"))
    })
}

/// Three minimal Gray files minus one minimal RGB file, at this JPEG's
/// tables. This zero-coefficient calibration includes the parameter records,
/// nested DC images, minimal entropy framing and whole-file completion. It
/// is a measured proxy penalty, not an estimate of a future sampled format.
fn tiny_proxy_penalty(entropy: imij::LossyEntropy, tq: [[u16; 64]; 3]) -> Result<usize> {
    let q = [0; 192];
    let mut proxy = 0;
    for table in tq {
        let cfg = coef_cfg(entropy, [table.map(f32::from); 3], true);
        let (file, oracle) = coef_file(8, 8, imij::Channels::Gray, &cfg, &q)?;
        if oracle != "ok" {
            return Err(format!("tiny proxy coefficient oracle: {oracle}").into());
        }
        retain(&file, "imi")?;
        proxy += file.len();
    }
    let cfg = coef_cfg(entropy, tq.map(|t| t.map(f32::from)), true);
    let (rgb, oracle) = coef_file(8, 8, imij::Channels::Rgb, &cfg, &q)?;
    if oracle != "ok" {
        return Err(format!("tiny RGB coefficient oracle: {oracle}").into());
    }
    retain(&rgb, "imi")?;
    proxy
        .checked_sub(rgb.len())
        .ok_or_else(|| "negative tiny proxy penalty".into())
}

/// Reference pixels of dequantized 4:4:4 coefficients: the f32 IDCT per
/// channel, then the signalled transform to RGB u8.
fn reference_rgb(dec: &zenjpeg::decoder::DecodedCoefficients, tables: &[[u16; 64]; 3]) -> Vec<u8> {
    let (bw, bh) = (dec.components[0].blocks_wide, dec.components[0].blocks_high);
    let (w, h) = (dec.width as usize, dec.height as usize);
    let stride = bw * 8;
    let mut planes = vec![vec![0f32; stride * bh * 8]; 3];
    for (c, comp) in dec.components.iter().enumerate().take(3) {
        for b in 0..comp.num_blocks() {
            let (bx, by) = (b % bw, b / bw);
            let src = comp.block(b);
            let mut deq = [0f32; 64];
            for (k, &v) in src.iter().enumerate() {
                let i = ZIGZAG[k];
                deq[i] = v as f32 * tables[c][i] as f32;
            }
            let px = idct8x8(&deq);
            for (i, &v) in px.iter().enumerate() {
                planes[c][(by * 8 + i / 8) * stride + bx * 8 + i % 8] = v;
            }
        }
    }
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let p = [
                planes[0][y * stride + x],
                planes[1][y * stride + x],
                planes[2][y * stride + x],
            ];
            for ch in 0..3 {
                let m = YCBCR_CENTERED[ch];
                let v = m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3];
                rgb[(y * w + x) * 3 + ch] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    rgb
}

/// Pixel oracle for a 4:4:4 three-plane file: decoded pixels vs the f32
/// IDCT reference through the signalled transform.
fn pixel_oracle(
    file: &[u8],
    dec: &zenjpeg::decoder::DecodedCoefficients,
    tq: &[[u16; 64]; 3],
) -> String {
    let Ok((info, pix)) = imij::decode(file) else {
        return "imij-decode-err".into();
    };
    let want = reference_rgb(dec, tq);
    if info.width != dec.width || info.height != dec.height || pix.len() != want.len() {
        return format!("dim {}x{}", info.width, info.height);
    }
    let (mut max, mut over) = (0i32, 0usize);
    for (&a, &b) in pix.iter().zip(&want) {
        let d = (a as i32 - b as i32).abs();
        max = max.max(d);
        over += (d > 1) as usize;
    }
    format!("max{max}over{over}")
}

struct Fields {
    out: String,
}
impl Fields {
    fn new() -> Self {
        Fields { out: String::new() }
    }
    fn num(&mut self, v: usize) {
        let _ = write!(self.out, "\t{v}");
    }
    fn flag(&mut self, v: bool) {
        let _ = write!(self.out, "\t{}", v as u8);
    }
    fn txt(&mut self, v: &str) {
        let _ = write!(self.out, "\t{}", v.replace(['\t', '\n'], " "));
    }
    fn blank(&mut self, n: usize) {
        for _ in 0..n {
            self.out.push('\t');
        }
    }
}

pub(crate) fn short(s: &str) -> String {
    s.chars().take(60).collect()
}

fn sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Retain every measured output by content hash when requested. The
/// environment supplies a study archive, with no private default path.
fn retain(data: &[u8], suffix: &str) -> Result<()> {
    if let Some(dir) = std::env::var_os("JPEG_SIZE_ARTIFACTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.{}", sha256(data), suffix));
        if !path.exists() {
            std::fs::write(path, data)?;
        }
    }
    Ok(())
}

/// Compare the source decoder with libjpeg-turbo's independent coefficient
/// reader. It exports natural-order coefficients of real component blocks;
/// zenjpeg additionally returns MCU padding, which is checked by the IMI
/// round-trip but has no visible image samples to compare here.
fn independent_coefficients(
    data: &[u8],
    dec: &zenjpeg::decoder::DecodedCoefficients,
) -> Result<String> {
    let Some(program) = std::env::var_os("JPEG_COEFFICIENT_ORACLE") else {
        return Ok("not-measured".into());
    };
    let dir = std::env::var_os("JPEG_SIZE_ARTIFACTS")
        .ok_or("JPEG_COEFFICIENT_ORACLE requires JPEG_SIZE_ARTIFACTS")?;
    retain(data, "jpg")?;
    let path = std::path::PathBuf::from(dir).join(format!("{}.jpg", sha256(data)));
    let result = std::process::Command::new(program).arg(path).output()?;
    if !result.status.success() {
        return Err(format!(
            "independent coefficient reader: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    let mut bytes = result.stdout.as_slice();
    fn u32le(bytes: &mut &[u8]) -> Result<usize> {
        let v = bytes.get(..4).ok_or("short coefficient oracle header")?;
        let n = u32::from_le_bytes(v.try_into()?) as usize;
        *bytes = &bytes[4..];
        Ok(n)
    }
    fn u16le(bytes: &mut &[u8]) -> Result<u16> {
        let v = bytes.get(..2).ok_or("short coefficient oracle data")?;
        let n = u16::from_le_bytes(v.try_into()?);
        *bytes = &bytes[2..];
        Ok(n)
    }
    if u32le(&mut bytes)? != dec.width as usize
        || u32le(&mut bytes)? != dec.height as usize
        || u32le(&mut bytes)? != dec.components.len()
    {
        return Ok("geometry-mismatch".into());
    }
    let mut differences = 0usize;
    for comp in &dec.components {
        let fields = [comp.id, comp.h_samp, comp.v_samp, comp.quant_table_idx];
        for field in fields {
            if u32le(&mut bytes)? != field as usize {
                return Ok("component-mismatch".into());
            }
        }
        let bw = u32le(&mut bytes)?;
        let bh = u32le(&mut bytes)?;
        if bw > comp.blocks_wide || bh > comp.blocks_high {
            return Ok("block-grid-mismatch".into());
        }
        let tq =
            dec.quant_tables[comp.quant_table_idx as usize].ok_or("missing component table")?;
        for q in tq {
            differences += (u16le(&mut bytes)? != q) as usize;
        }
        for y in 0..bh {
            for x in 0..bw {
                let block = comp.block_at(x, y);
                let mut natural = [0i16; 64];
                for (k, &v) in block.iter().enumerate() {
                    natural[ZIGZAG[k]] = v;
                }
                for v in natural {
                    differences += (u16le(&mut bytes)? as i16 != v) as usize;
                }
            }
        }
    }
    if !bytes.is_empty() {
        return Err("trailing coefficient oracle data".into());
    }
    Ok(if differences == 0 {
        "ok".into()
    } else {
        format!("mismatch:{differences}")
    })
}

// Columns after `path sha256`:
// w h sampling sof progressive arithmetic precision restart scans
// meta_bytes tail_bytes exif_ycbcr dqt16 coef
// orig stripped jpeg_seq jpeg_prog jxl jxl_via
// pxy0_rl pxy1_rl pxy2_rl proxy_rl imij3_rl
// pxy0_sh pxy1_sh pxy2_sh proxy_sh imij3_sh
// imij_pix oracle px_oracle
fn measure(data: &[u8], f: &mut Fields) -> Result<()> {
    let h = jpeg_head(data)?;
    let class = sampling(&h);
    let progressive = matches!(h.sof, 0xC2 | 0xCA | 0xC5 | 0xC6);
    let transcodable = h.precision == 8
        && !h.arithmetic
        && matches!(h.sof, 0xC0 | 0xC1 | 0xC2)
        && matches!(class.as_str(), "gray" | "444" | "420");

    f.num(h.width as usize);
    f.num(h.height as usize);
    f.txt(&class);
    f.num(h.sof as usize);
    f.flag(progressive);
    f.flag(h.arithmetic);
    f.num(h.precision as usize);
    f.num(h.restart.map_or(0, |r| r as usize));
    f.num(h.scans);
    f.num(h.meta_bytes);
    f.num(h.tail_bytes);
    f.txt(&h.exif_ycbcr.map_or("absent".into(), |v| format!("{v}")));
    f.flag(h.dqt16);

    let dec = match zenjpeg::decoder::DecodeConfig::new()
        .decode_coefficients(&h.stripped, enough::Unstoppable)
    {
        Ok(d) => Some(d),
        Err(e) => {
            f.txt(&format!("err:{}", short(&e.to_string())));
            None
        }
    };
    if dec.is_some() {
        f.txt("ok");
    }

    f.num(data.len());
    f.num(h.stripped.len());

    let mut repack_base: Option<Vec<u8>> = None;
    let mut repack_checks = Vec::new();
    for mode in [
        zenjpeg::lossless::OutputMode::Sequential,
        zenjpeg::lossless::OutputMode::Progressive,
    ] {
        match zenjpeg::lossless::restructure(
            &h.stripped,
            &zenjpeg::lossless::RestructureConfig {
                output_mode: mode,
                restart_interval: zenjpeg::lossless::RestartInterval::None,
                transform: None,
            },
            enough::Unstoppable,
        ) {
            Ok(v) => {
                // Restructuring can add its own JFIF marker. Apply the same
                // metadata policy to the comparator output as to the input.
                let v = jpeg_head(&v)?.stripped;
                retain(&v, "jpg")?;
                if let Some(d) = &dec {
                    repack_checks.push(independent_coefficients(&v, d)?);
                }
                if mode == zenjpeg::lossless::OutputMode::Sequential {
                    repack_base = Some(v.clone());
                }
                f.num(v.len());
            }
            Err(e) => f.txt(&format!("err:{}", short(&e.to_string()))),
        }
    }

    // Try the source directly, then the sequential repack if unavailable.
    // The recorded route identifies which coefficient-preserving input won.
    let jxl_cfg = jxl_encoder::LosslessConfig::new().with_effort(9);
    let mut jxl_via = "err";
    let jxl = match jxl_cfg.encode_jpeg_transcode(&h.stripped) {
        Ok(v) => {
            jxl_via = "direct";
            Some(v)
        }
        Err(_) => repack_base
            .as_ref()
            .and_then(|b| jxl_cfg.encode_jpeg_transcode(b).ok())
            .inspect(|_| jxl_via = "via-repack"),
    };
    match jxl {
        Some(v) => {
            retain(&v, "jxl")?;
            f.num(v.len());
            f.txt(jxl_via);
        }
        None => {
            f.txt("");
            f.txt(jxl_via);
        }
    }

    // IMI arms: the same coefficients at the JPEG's own tables.
    let mut oracles = Vec::new();
    let mut px_oracle = String::new();
    let mut px_oracle_sh = String::new();
    let mut rl_pixels = None;
    let mut tiny_penalties = Vec::new();
    if transcodable && dec.is_some() {
        let dec = dec.as_ref().unwrap();
        let tq = |ci: usize| -> Result<[u16; 64]> {
            let idx = dec.components[ci].quant_table_idx as usize;
            dec.quant_tables
                .get(idx)
                .copied()
                .flatten()
                .ok_or_else(|| format!("missing quant table for comp {ci}").into())
        };
        let mut tq3 = [tq(0)?, tq(0)?, tq(0)?];
        if dec.components.len() >= 3 {
            tq3[1] = tq(1)?;
            tq3[2] = tq(2)?;
        }
        let f32t = |t: [u16; 64]| t.map(|v| v as f32);
        for (tag, ent) in [
            ("rl", imij::LossyEntropy::RunLevel),
            ("sh", imij::LossyEntropy::SplitHuf),
        ] {
            let mut proxy_sum = 0usize;
            for (ci, comp) in dec.components.iter().enumerate() {
                let mut q = vec![0i32; comp.blocks_wide * comp.blocks_high * 192];
                scatter_comp(&mut q, 0, &comp.coeffs);
                let cfg = coef_cfg(ent, [tq3[ci].map(|v| v as f32); 3], true);
                match coef_file(
                    (comp.blocks_wide * 8) as u32,
                    (comp.blocks_high * 8) as u32,
                    imij::Channels::Gray,
                    &cfg,
                    &q,
                ) {
                    Ok((file, o)) => {
                        retain(&file, "imi")?;
                        proxy_sum += file.len();
                        f.num(file.len());
                        oracles.push(format!("pxy{ci}{tag}:{o}"));
                    }
                    Err(e) => {
                        f.txt(&format!("err:{}", short(&e.to_string())));
                        oracles.push(format!("pxy{ci}{tag}:enc-err"));
                    }
                }
            }
            for _ in dec.components.len()..3 {
                f.txt("");
            }
            f.num(proxy_sum);
            if class == "444" {
                let bw = dec.components[0].blocks_wide;
                let bh = dec.components[0].blocks_high;
                let mut q = vec![0i32; bw * bh * 192];
                for (ch, comp) in dec.components.iter().enumerate().take(3) {
                    scatter_comp(&mut q, ch, &comp.coeffs);
                }
                let cfg = coef_cfg(ent, tq3.map(f32t), true);
                match coef_file(h.width, h.height, imij::Channels::Rgb, &cfg, &q) {
                    Ok((file, o)) => {
                        retain(&file, "imi")?;
                        if ent == imij::LossyEntropy::RunLevel {
                            px_oracle = pixel_oracle(&file, dec, &tq3);
                            rl_pixels = Some(imij::decode(&file)?.1);
                        } else {
                            px_oracle_sh = pixel_oracle(&file, dec, &tq3);
                            if Some(imij::decode(&file)?.1) != rl_pixels {
                                oracles.push("coder-pixels:mismatch".into());
                            }
                        }
                        f.num(file.len());
                        oracles.push(format!("imij3{tag}:{o}"));
                    }
                    Err(e) => {
                        f.txt(&format!("err:{}", short(&e.to_string())));
                        oracles.push(format!("imij3{tag}:enc-err"));
                    }
                }
            } else {
                f.txt("");
            }
            tiny_penalties.push(if class == "gray" {
                0
            } else {
                tiny_proxy_penalty(ent, tq3)?
            });
        }
    } else {
        f.blank(10);
    }

    // Fallback arm for non-transcodable but pixel-decodable files: IMI
    // lossless of decoded RGB.
    if !transcodable {
        let mut dc = zenjpeg::decoder::DecodeConfig::new();
        dc.output_format = Some(zenjpeg::decoder::PixelFormat::Rgb);
        match dc.decode(data, enough::Unstoppable) {
            Ok(res) => match res.pixels_u8() {
                Some(pix) => {
                    let ii = imij::ImageInfo::new(
                        res.width,
                        res.height,
                        imij::Channels::Rgb,
                        imij::SampleType::U8,
                    );
                    match imij::ImiEncoderConfig::default().encode_samples(
                        imij::ImageRef::new(pix, ii.row_bytes(), &ii),
                        &imij::Metadata::default(),
                    ) {
                        Ok(v) => {
                            verify_fallback(&v, &ii, pix)?;
                            f.num(v.len());
                        }
                        Err(e) => f.txt(&format!("err:{}", short(&e.to_string()))),
                    }
                }
                None => f.txt("no-pix"),
            },
            Err(e) => f.txt(&format!("err:{}", short(&e.to_string()))),
        }
    } else {
        f.txt("");
    }

    let oracle = if oracles.is_empty() {
        String::new()
    } else if oracles.iter().all(|o| o.ends_with(":ok")) {
        "ok".into()
    } else {
        oracles.join(",")
    };
    f.txt(&oracle);
    f.txt(&px_oracle);
    f.txt(&match &dec {
        Some(d) => independent_coefficients(&h.stripped, d)?,
        None => "not-measured".into(),
    });
    f.txt(&repack_checks.join(","));
    let mut coefficient_digests = Vec::new();
    let mut table_digests = Vec::new();
    if let Some(d) = &dec {
        for comp in &d.components {
            let mut digest = Sha256::new();
            for block in comp.coeffs.chunks_exact(64) {
                let mut natural = [0i16; 64];
                for (k, &v) in block.iter().enumerate() {
                    natural[ZIGZAG[k]] = v;
                }
                for v in natural {
                    digest.update(v.to_le_bytes());
                }
            }
            coefficient_digests.push(format!("{:x}", digest.finalize()));
            let mut digest = Sha256::new();
            for v in d.quant_tables[comp.quant_table_idx as usize].ok_or("missing table")? {
                digest.update(v.to_le_bytes());
            }
            table_digests.push(format!("{:x}", digest.finalize()));
        }
    }
    f.txt(&coefficient_digests.join(","));
    f.txt(&table_digests.join(","));
    f.txt(&px_oracle_sh);
    for i in 0..2 {
        if let Some(&n) = tiny_penalties.get(i) {
            f.num(n);
        } else {
            f.txt("");
        }
    }
    let quality = zenjpeg::detect::probe(data).ok();
    f.txt(
        &quality
            .as_ref()
            .map(|p| format!("{:?}", p.quality.scale))
            .unwrap_or_default(),
    );
    f.txt(
        &quality
            .as_ref()
            .map(|p| format!("{}", p.quality.value))
            .unwrap_or_default(),
    );
    f.txt(
        &quality
            .as_ref()
            .map(|p| format!("{:?}", p.quality.confidence))
            .unwrap_or_default(),
    );
    Ok(())
}

fn one(path: &str, scan: bool) -> Result<String> {
    let data = std::fs::read(path)?;
    if scan {
        let h = jpeg_head(&data)?;
        return Ok(format!(
            "{path}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            sha256(&data),
            h.width,
            h.height,
            sampling(&h),
            h.sof,
            h.precision,
            h.restart.unwrap_or(0),
            h.scans,
            h.meta_bytes,
            h.tail_bytes,
            h.exif_ycbcr.map_or("absent".into(), |v| format!("{v}")),
            h.dqt16 as u8,
        ));
    }
    let mut f = Fields::new();
    match measure(&data, &mut f) {
        Ok(()) => Ok(format!("{path}\t{}{}", sha256(&data), f.out)),
        Err(e) => Err(e),
    }
}

/// zenjpeg's default (libjpeg-turbo-exact) decode of `path` as binary P6/P5 —
/// the reference the phase-2 oracle compares `djpeg` output against.
fn ppm(path: &str, out: &str) -> Result<()> {
    let data = std::fs::read(path)?;
    let h = jpeg_head(&data)?;
    // djpeg semantics: the stripped stream carries no EXIF, so no
    // auto-orientation applies; PGM for one-component files.
    let mut dc = zenjpeg::decoder::DecodeConfig::new();
    if h.ncomp == 1 {
        dc.output_format = Some(zenjpeg::decoder::PixelFormat::Gray);
    }
    let res = dc
        .decode(&h.stripped, enough::Unstoppable)
        .map_err(|e| format!("decode: {e}"))?;
    let (pix, fmt) = (res.pixels_u8(), res.format);
    let Some(pix) = pix else {
        return Err("no u8 pixels".into());
    };
    use zenjpeg::decoder::PixelFormat as P;
    let (hdr, body) = match fmt {
        P::Gray => (
            format!("P5\n{} {}\n255\n", res.width, res.height),
            pix.to_vec(),
        ),
        P::Rgb => (
            format!("P6\n{} {}\n255\n", res.width, res.height),
            pix.to_vec(),
        ),
        P::Rgba | P::Bgra | P::Bgrx => {
            let mut v = Vec::with_capacity(res.width as usize * res.height as usize * 3);
            let bgr = fmt == P::Bgra;
            for p in pix.chunks_exact(4) {
                if bgr {
                    v.extend_from_slice(&[p[2], p[1], p[0]]);
                } else {
                    v.extend_from_slice(&p[..3]);
                }
            }
            (format!("P6\n{} {}\n255\n", res.width, res.height), v)
        }
        P::Bgr => (format!("P6\n{} {}\n255\n", res.width, res.height), {
            let mut v = pix.to_vec();
            for p in v.chunks_exact_mut(3) {
                p.swap(0, 2);
            }
            v
        }),
        _ => return Err(format!("unsupported pixel format {fmt:?}").into()),
    };
    let mut f = std::fs::File::create(out)?;
    use std::io::Write as _;
    f.write_all(hdr.as_bytes())?;
    f.write_all(&body)?;
    Ok(())
}

/// A fallback preserves the decoded pixel reference exactly. The JPEG's
/// coefficients remain outside this route's contract.
fn verify_fallback(file: &[u8], expected: &imij::ImageInfo, pixels: &[u8]) -> Result<()> {
    let (info, decoded) = imij::decode(file)?;
    if info.width != expected.width
        || info.height != expected.height
        || info.channels != expected.channels
        || info.sample != expected.sample
        || decoded != pixels
    {
        return Err("fallback lossless roundtrip differs from decoded JPEG pixels".into());
    }
    Ok(())
}

fn fallback_check(out: &str, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err("fallback check requires at least one JPEG".into());
    }
    let mut rows = String::from(
        "sha256\tsampling\tprecision\tarithmetic\twidth\theight\timij_bytes\tpixel_sha256\troundtrip\n",
    );
    for path in paths {
        let data = std::fs::read(path)?;
        let head = jpeg_head(&data)?;
        let class = sampling(&head);
        if head.precision == 8
            && !head.arithmetic
            && matches!(head.sof, 0xC0 | 0xC1 | 0xC2)
            && matches!(class.as_str(), "gray" | "444" | "420")
        {
            return Err(
                format!("fallback check received coefficient-eligible JPEG: {path}").into(),
            );
        }
        let mut cfg = zenjpeg::decoder::DecodeConfig::new();
        cfg.output_format = Some(zenjpeg::decoder::PixelFormat::Rgb);
        let decoded = cfg.decode(&data, enough::Unstoppable)?;
        let pixels = decoded.pixels_u8().ok_or("fallback has no RGB8 pixels")?;
        let info = imij::ImageInfo::new(
            decoded.width,
            decoded.height,
            imij::Channels::Rgb,
            imij::SampleType::U8,
        );
        let file = imij::ImiEncoderConfig::default().encode_samples(
            imij::ImageRef::new(pixels, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )?;
        verify_fallback(&file, &info, pixels)?;
        writeln!(
            rows,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\tok",
            sha256(&data),
            class,
            head.precision,
            u8::from(head.arithmetic),
            info.width,
            info.height,
            file.len(),
            sha256(pixels)
        )?;
        // Save each completed witness so a later failure keeps prior results.
        std::fs::write(out, &rows)?;
    }
    eprintln!("verified {} fallback lossless roundtrips", paths.len());
    Ok(())
}

pub(crate) fn command(a: &[String]) -> Result<()> {
    if a[1] == "jpegsize-fallback-check" {
        if a.len() < 4 {
            return Err("jpegsize-fallback-check OUT.tsv FILE.jpg...".into());
        }
        return fallback_check(&a[2], &a[3..]);
    }
    if a[1] == "jpegsize-ppm" {
        if a.len() < 4 {
            return Err("jpegsize-ppm FILE.jpg OUT.ppm".into());
        }
        return ppm(&a[2], &a[3]);
    }
    let scan = a[1] == "jpegsize-scan";
    if a.len() < 4 {
        return Err("jpegsize OUT.tsv FILE... | jpegsize-scan OUT.tsv FILE...".into());
    }
    let head = if scan {
        "path\tsha256\tw\th\tsampling\tsof\tprecision\trestart\tscans\tmeta_bytes\ttail_bytes\texif_ycbcr\tdqt16"
    } else {
        "path\tsha256\tw\th\tsampling\tsof\tprogressive\tarithmetic\tprecision\trestart\tscans\tmeta_bytes\ttail_bytes\texif_ycbcr\tdqt16\tcoef\torig\tstripped\tjpeg_seq\tjpeg_prog\tjxl\tjxl_via\tpxy0_rl\tpxy1_rl\tpxy2_rl\tproxy_rl\timij3_rl\tpxy0_sh\tpxy1_sh\tpxy2_sh\tproxy_sh\timij3_sh\timij_pix\toracle\tpx_oracle\tsource_oracle\trepack_oracle\tcoefficient_digests\ttable_digests\tpx_oracle_sh\ttiny_penalty_rl\ttiny_penalty_sh\tquality_scale\tquality_estimate\tquality_confidence"
    };
    let mut out = String::from(head);
    out.push('\n');
    for p in &a[3..] {
        match one(p, scan) {
            Ok(row) => {
                out.push_str(&row);
                out.push('\n');
            }
            Err(e) => {
                eprintln!("{p}: {e}");
                let _ = writeln!(out, "{p}\tERR\t{}", short(&e.to_string()));
            }
        }
    }
    std::fs::write(&a[2], &out)?;
    eprintln!("wrote {}", &a[2]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_triangle_matches_measured_float_arithmetic_with_strides() {
        for width in 1usize..=33 {
            for height in 1usize..=9 {
                let packed: Vec<f32> = (0..width * height)
                    .map(|i| (((i * 97 + 11) % 1021) as f32 - 510.0) * 0.25)
                    .collect();
                let stride = width + 5;
                let mut input = vec![f32::NAN; stride * height];
                for y in 0..height {
                    input[y * stride..y * stride + width]
                        .copy_from_slice(&packed[y * width..(y + 1) * width]);
                }
                for out_width in [width * 2 - 1, width * 2] {
                    for out_height in [height * 2 - 1, height * 2] {
                        let reference = zenjpeg::decode::upsample::upsample_libjpeg_f32(
                            &packed, width, height, out_width, out_height, 2, 2,
                        );
                        let out_stride = out_width + 7;
                        let mut output = vec![f32::NAN; out_stride * out_height];
                        imij::lab::jpeg_triangle(
                            &input,
                            width,
                            height,
                            stride,
                            &mut output,
                            out_width,
                            out_height,
                            out_stride,
                        )
                        .unwrap();
                        for y in 0..out_height {
                            for x in 0..out_width {
                                assert_eq!(
                                    output[y * out_stride + x].to_bits(),
                                    reference[y * out_width + x].to_bits(),
                                    "{width}x{height} to {out_width}x{out_height}, {x},{y}"
                                );
                            }
                            assert!(
                                output[y * out_stride + out_width..(y + 1) * out_stride]
                                    .iter()
                                    .all(|v| v.is_nan())
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn component_proxy_preserves_jpeg_level_shift_and_dc_table() {
        for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
            for dc in [-8, 0, 8] {
                let cfg = coef_cfg(entropy, [[17.0; 64]; 3], true);
                let mut q = [0; 192];
                q[0] = dc;
                let (file, oracle) = coef_file(8, 8, imij::Channels::Gray, &cfg, &q).unwrap();
                assert_eq!(oracle, "ok");
                // JPEG's constant-block identity is DC * table[0] / 8 + 128.
                let expected = (128 + dc * 17 / 8) as u8;
                let (_, pixels) = imij::decode(&file).unwrap();
                assert_eq!(pixels, vec![expected; 64]);
            }
        }
    }

    #[test]
    fn centered_ycbcr_bits() {
        let bits: [[u32; 4]; 3] = YCBCR_CENTERED.map(|r| r.map(f32::to_bits));
        assert_eq!(
            bits,
            [
                [0x3f800000, 0x00000000, 0x3fb374bc, 0x43000000],
                [0x3f800000, 0xbeb03298, 0xbf36d19e, 0x43000000],
                [0x3f800000, 0x3fe2d0e5, 0x00000000, 0x43000000],
            ]
        );
    }
}
