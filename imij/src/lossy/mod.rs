//! Lossy mode: 8x8 DCT blocks in independent stripe/column-group units.
//! Design and measurements: `docs/LOSSY.md`.
//!
//! The `DATA` payload (SPEC 2.8) starts with the stripe/group geometry, the
//! parameter presence mask and quantizer scale with the parameters that
//! differ from their defaults (seed byte, entropy mode, matrix, DC steps,
//! color scales, floor, multipliers), the AC entropy tables, the head record
//! list (unit table hint, exact blocks, AQ, keep blocks, 32-bit DC, Lossy
//! 0.4 records, channel layouts) and the nested lossless DC, alpha and
//! exact heads. Unit records (SPEC 2.7: `id, len, payload`) follow in
//! stripe-major, column-group order: a DC record when a new nested DC stripe
//! begins, alpha, AC, then exact. The internal color is the header's
//! `coding`. See `parse_lossy_head_checked`, `parse_lossy` and docs/LOSSY.md.
//!
//! DC uses U16 or I32 as required; alpha preserves the original samples.
//! AC supports run-level rANS and split-Huffman; packed rANS and split rANS
//! are retired (SPEC 2.8 reserves wire values 1 and 2; see
//! [`coef::LossyEntropy`]).
//! Units are independent given their shared models
//! and corresponding DC/alpha data. Coded 8x8 blocks extend to the
//! right/bottom edge; output is clipped to the header's exact dimensions.
//!
//! U32 color is full-range RGB code data, with independently declared 1..32
//! significant bits. YCoCg uses normalized 0..255 units; XYB uses the declared
//! sRGB/PQ/HLG transfer to linear light. Conversion never allocates a 2^32
//! table or truncates through U16. The existing f32 transforms/DCT and bounded
//! inverse-transfer interpolation are lossy even at maximum quality: U32
//! storage is not a promise of 32-bit color precision. Alpha remains lossless.

#[cfg(feature = "encode")]
pub(crate) mod adaptive_rdo;
#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) mod block_lab;
#[cfg(feature = "encode")]
pub(crate) mod bounded;
#[cfg(feature = "lab")]
pub mod chroma_lab;
mod coding_defaults;
pub(crate) mod coef;
pub(crate) mod color;
#[cfg(target_arch = "x86_64")]
mod color_v3;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod color_v4x;
mod compact;
pub(crate) mod dc_coverage;
pub(crate) mod dct;
pub(crate) mod dering;
#[cfg(feature = "encode")]
pub(crate) mod enc_simd;
#[cfg(target_arch = "x86_64")]
mod entropy_avx2;
#[cfg(target_arch = "x86_64")]
mod entropy_avx512;
#[cfg(target_arch = "aarch64")]
mod entropy_neon;
pub(crate) mod exact;
mod local_ans;
#[cfg(feature = "encode")]
mod model_candidates;
mod model_refs;
mod reconstruction;
pub(crate) mod rows;
mod xyb_power;
use reconstruction::Reconstruction;
pub(crate) mod v4;
#[cfg(feature = "std")]
#[cfg(feature = "encode")]
pub(crate) use bounded::bounded_working_bytes;
pub(crate) use rows::{LossyStream, OwnedRecords};
mod superblock;
#[cfg(feature = "std")]
pub(crate) use rows::read_lossy;
pub(crate) use superblock::map::{decode_v4_partitions, joint as joint_settings};
// Callers are gated on wasm32 (the portable kernels' target); `test` also
// compiles it on other arches so its hostile-input sweeps exercise the
// generic scalar tier, and the items are dead there outside tests.
#[cfg(any(target_arch = "wasm32", all(test, feature = "encode")))]
#[allow(dead_code)]
mod entropy_portable;
mod envelope;
pub(crate) mod huf;
#[cfg(feature = "native-jpeg")]
pub(crate) mod jpeg;
mod layout;
mod small;
#[cfg(feature = "lab")]
pub(crate) mod small_lab;
mod subblock;
pub use layout::{ChannelSampling, LossyChannelLayout, LossyTransform};
#[cfg(feature = "lab")]
mod aq_field_lab;
#[cfg(target_arch = "x86_64")]
mod kernel_choice;
#[cfg(all(feature = "lab", feature = "std"))]
pub(crate) mod lanes_lab;
mod quality_scale;
mod quality_scale_tables;
mod rawbits;
#[cfg(feature = "lab")]
mod rdoq_lab;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod recon_v4;
#[cfg(feature = "encode")]
pub(crate) mod trellis;
#[cfg(all(test, feature = "encode"))]
#[cfg(feature = "encode")]
mod trellis_base;
#[cfg(feature = "lab")]
pub use trellis::LossyTrellis;
mod config;
#[cfg(feature = "encode")]
mod encode;
#[cfg(feature = "encode")]
mod entropy_encode;
#[cfg(feature = "encode")]
mod partition;
mod qparams;
mod simd;
#[cfg(target_arch = "x86_64")]
mod split_avx2;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
mod split_avx512;
mod srgb_table;
mod transfer;
#[cfg(feature = "encode")]
use config::BiasSums;
pub use config::{ColorTransform, ReconBias};
#[cfg(feature = "lab")]
pub use config::{LossyAqField, LossyLabSwitches, SmallLab};
#[cfg(feature = "encode")]
pub(crate) use config::{LossyConfig, adaptive_enabled, adaptive_mode};
#[cfg(all(test, feature = "encode"))]
pub(crate) use encode::encode_lossy;
#[cfg(all(feature = "lab", feature = "encode"))]
pub(crate) use encode::encode_lossy_coefs;
#[cfg(all(test, feature = "lab", feature = "encode"))]
use encode::finish_lossy_frame;
#[cfg(all(feature = "encode", feature = "native-jpeg"))]
use encode::finish_quantized_coefficients;
#[cfg(feature = "encode")]
pub(crate) use encode::{
    encode_lossy_frame, encode_lossy_frame_with_progress, encode_lossy_with_progress,
};
#[cfg(all(test, feature = "encode"))]
use entropy_encode::ans_tables;
#[cfg(feature = "encode")]
use entropy_encode::{
    Coded, HuffmanStripe, TokenHistogram, ans_tables_for_decisions, encode_ans_stripes,
    encode_huffman_stripes, huffman_tables, seed_bank, seed_exact, split_units,
    stripe_counts_widths, stripe_raw_split, stripe_symbols, stripe_tokens,
};
#[cfg(feature = "encode")]
use partition::{exact_aq_to_selector, exact_aq_to_wire, small_on, superblocks_on};
use qparams::QParams;
pub(crate) use qparams::default_dc_step;
pub use quality_scale::{ButteraugliNorm, LossyQuality};

#[cfg(all(not(feature = "std"), feature = "encode"))]
use num_traits::Float as _;

#[cfg(feature = "encode")]
use alloc::vec;
use alloc::vec::Vec;

#[cfg(feature = "encode")]
use crate::Metadata;
use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;
#[cfg(feature = "encode")]
use crate::put_varint;
#[cfg(feature = "encode")]
use crate::rans::{EncodeTables, Sym, encode_lanes_tables};
use crate::rans::{LOG_M, M, NL, RANS_L, Words};
use crate::{ByteReader, Channels, Error, ImageInfo, SampleType};
pub use coef::LossyEntropy;
#[cfg(feature = "encode")]
use coef::rl_token;
use coef::{NCH, Tables, ZIGZAG, ac_ctx, eob_ctx, rl_value};
pub use color::LossyColor;
#[cfg(feature = "encode")]
use how_far::Pulse;

/// Lossy head record ids (SPEC 2.8); id 10 is the unit table hint.
/// Exact blocks (feature `exact_blocks`): block code 1 = exact block.
const H_EXACT: u64 = crate::registry::HEAD_EXACT;
/// AQ: the DC image's fourth channel holds AC step multipliers a/64.
const H_AQ: u64 = crate::registry::HEAD_AQ;
/// Keep blocks (inter frames): block code 0 keeps the reference block.
const H_KEEP: u64 = crate::registry::HEAD_KEEP;
/// The DC image's samples are i32 instead of u16.
const H_DC32: u64 = crate::registry::HEAD_DC32;
/// Lossy 0.4 explicit quantization tables (feature `explicit_tables`).
const H_EXPLICIT: u64 = crate::registry::HEAD_EXPLICIT;
/// Lossy 0.4 reconstruction bias (feature `recon_bias`).
const H_BIAS: u64 = crate::registry::HEAD_BIAS;
/// Lossy 0.4 color transform (feature `color_transform`).
const H_TRANSFORM: u64 = crate::registry::HEAD_TRANSFORM;
/// Per-plane channel layouts (transform size, sampling).
const H_LAYOUTS: u64 = crate::registry::HEAD_LAYOUTS;

pub(crate) use envelope::MATRIX_EXPLICIT;
#[cfg(feature = "encode")]
pub(crate) use envelope::MATRIX_LAST_PRESET;

/// Entropy-work counts of the quantized image: [run-level tokens, coded
/// groups (split-Huffman), block-channels with any coded group].
#[cfg(feature = "encode")]
fn side_stats(stripes: &[StripeCoefs], bw: usize, threads: usize) -> [u64; 3] {
    let per: Vec<[u64; 3]> =
        crate::par::par_map(stripes.iter().collect(), threads, |_, sc: &StripeCoefs| {
            let mut n = 0u64;
            stripe_tokens(sc, bw, Group::ALL, |_, _, _, _, _| n += 1);
            let (mut groups, mut coded) = (0u64, 0u64);
            for &m in &sc.nz {
                let ac = m & !1;
                if ac != 0 {
                    groups += (63 - ac.leading_zeros() as u64).div_ceil(4);
                    coded += 1;
                }
            }
            [n, groups, coded]
        });
    per.iter()
        .fold([0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]])
}

/// [`side_stats`] totals when the token count is already known: column-
/// group splitting preserves the per-block `nz` multiset, so groups and
/// coded totals are a linear `nz` scan with no coefficient walk.
#[cfg(feature = "encode")]
fn side_stats_lanes(stripes: &[StripeCoefs], tokens: u64) -> [u64; 3] {
    let (mut groups, mut coded) = (0u64, 0u64);
    for sc in stripes {
        for &m in &sc.nz {
            let ac = m & !1;
            if ac != 0 {
                groups += (63 - ac.leading_zeros() as u64).div_ceil(4);
                coded += 1;
            }
        }
    }
    [tokens, groups, coded]
}

/// Images up to this many pixels code both ways under a tier and keep
/// split-Huffman when it is smaller.
#[cfg(feature = "encode")]
const SMALL_IMAGE: usize = 1 << 16;

/// Predicted decode time of split-Huffman over run-level from the
/// encoder's counts `[tokens, groups, coded block-channels]`: per-mode
/// linear models fitted to 1-thread decode times on Zen 4 (7900X), 344
/// image / quality pairs from 64x64 crops to 35 MP, photos and graphics,
/// q10-q95; held-out median ratio error 0.035-0.10 (docs/LOSSY.md E17,
/// imij-cmp/fit_cost.py).
#[cfg(feature = "encode")]
fn predicted_huf_ratio(px: f64, st: &[u64; 3]) -> f64 {
    let [tok, grp, bc] = st.map(|v| v as f64);
    let rl = 36_200.0 + 1.624 * px + 4.183 * tok;
    let sh = 26_200.0 + 1.749 * px + 0.842 * grp + 41.416 * bc;
    sh / rl
}

/// Largest predicted time ratio at which a tier takes split-Huffman.
#[cfg(feature = "encode")]
fn huf_max_ratio(speed: crate::DecodeSpeed) -> Option<f64> {
    use crate::DecodeSpeed as D;
    match speed {
        D::Compact => None,
        D::Balanced => Some(0.85),
        D::Fast | D::Fastest => Some(0.93),
    }
}

/// Pixel formats the lossy mode accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputKind {
    U8,
    U16,
    U32,
    F16,
    F32,
}

impl InputKind {
    /// Sample kind of a supported layout (gray, gray+alpha, RGB, RGBA).
    fn of(info: &ImageInfo) -> Option<InputKind> {
        if info.channels.is_native_model() {
            return None;
        }
        match info.sample {
            SampleType::U8 => Some(InputKind::U8),
            SampleType::U16 => Some(InputKind::U16),
            SampleType::U32 => Some(InputKind::U32),
            SampleType::F16 => Some(InputKind::F16),
            SampleType::F32 => Some(InputKind::F32),
            _ => None,
        }
    }
}

/// Color samples of a pixel: gray replicates its one channel.
fn color_index(ch: Channels) -> [usize; 3] {
    if matches!(ch, Channels::Gray | Channels::GrayAlpha) {
        [0, 0, 0]
    } else {
        [0, 1, 2]
    }
}

/// Non-finite input samples are coded as 0.
#[inline]
#[cfg(feature = "encode")]
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// Deinterleave one source row and extend its last pixel to the coded width.
/// Typed sample iteration avoids repeated byte-offset arithmetic and slice
/// bounds checks in the per-component loop.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn fill_color_row<const BPS: usize>(
    row: &[u8],
    planes: &mut [f32],
    pw: usize,
    y: usize,
    w: usize,
    channels: usize,
    indices: [usize; 3],
    read: impl Fn(usize, &[u8; BPS]) -> f32,
) {
    let samples = row.as_chunks::<BPS>().0;
    for (c, channel) in indices.into_iter().enumerate() {
        let dst = &mut planes[(c * 8 + y) * pw..][..pw];
        for (sample, value) in samples
            .iter()
            .skip(channel)
            .step_by(channels)
            .zip(&mut dst[..w])
        {
            *value = read(c, sample);
        }
        let last = dst[w - 1];
        dst[w..].fill(last);
    }
}

/// The encoder's row pipeline (`fill_block_row` in the encoder):
/// `rows(by, planes, extremes)` writes block row `by`'s color-converted (and
/// deringed) planes for every column of the image, `NCH * 8 * pw` samples,
/// plane-major, and the row's white/black blocks into `extremes` (integer
/// inputs when needed; otherwise left untouched). Shared with the passes that
/// recompute rows (the trellis, lab RDOQ).
#[cfg(feature = "encode")]
type RowPlanes<'a> = &'a (dyn Fn(usize, &mut [f32], &mut Vec<(u64, bool)>) + Sync);

/// Per block of one block row: the mask of white samples (every color
/// channel at its maximum code, raster order) and whether every sample has
/// every color channel at code 0. Columns and rows past the image edge
/// repeat the edge, as the planes do.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn block_extremes(
    g: &Geom,
    pixels: &[u8],
    stride: usize,
    y0: usize,
    nci: usize,
    ci: [usize; 3],
    kind: InputKind,
    white_code: &[u32; 3],
    out: &mut Vec<(u64, bool)>,
) {
    let bps = match kind {
        InputKind::U8 => 1,
        _ => 2,
    };
    let code = |row: &[u8], x: usize, c: usize| -> u32 {
        let at = (x * nci + ci[c]) * bps;
        if bps == 1 {
            row[at] as u32
        } else {
            u16::from_ne_bytes([row[at], row[at + 1]]) as u32
        }
    };
    out.clear();
    for bx in 0..g.bw {
        let mut white = 0u64;
        let mut black = true;
        for y in 0..8 {
            let row = &pixels[(y0 + y).min(g.h - 1) * stride..];
            for x in 0..8 {
                let sx = (bx * 8 + x).min(g.w - 1);
                let c = [code(row, sx, 0), code(row, sx, 1), code(row, sx, 2)];
                if c == *white_code {
                    white |= 1 << (y * 8 + x);
                }
                black &= c == [0, 0, 0];
            }
        }
        out.push((white, black));
    }
}

/// Deringing for one block row (dering.rs): reshapes each block's white
/// samples in plane 0.
#[cfg(feature = "encode")]
fn dering_row(plane0: &mut [f32], pw: usize, extremes: &[(u64, bool)], cap: f32) {
    for (bx, &(white, _)) in extremes.iter().enumerate() {
        if white == 0 {
            continue;
        }
        let i = white.trailing_zeros() as usize;
        let top = plane0[(i / 8) * pw + bx * 8 + i % 8];
        // 31/255 of the white level, as mozjpeg's 31 of 255 codes.
        let cap = cap.min(top.abs() * (31.0 / 255.0));
        dering::dering_block(plane0, pw, bx * 8, white, top, cap);
    }
}

/// DC quantization for one channel of a block row, replacing the shared
/// quantizer's dead-zone rounding (which pulls every block mean toward zero:
/// flat white came back as 253-254 in YCoCg) with rounding to nearest. With
/// `extremes` (luma, default layout), a block whose source is entirely at
/// the maximum code rounds its DC up and one entirely at code 0 rounds it
/// down, so the decoder's clamp returns exact white and black.
#[cfg(feature = "encode")]
fn dc_row(
    coefs: &[f32],
    bw: usize,
    ch: usize,
    inv_step: f32,
    extremes: Option<&[(u64, bool)]>,
    q: &mut [i32],
    nz: &mut [u64],
) {
    const CAP: f32 = ((1 << 22) - 1) as f32;
    for bx in 0..bw {
        let c = coefs[bx * 64] * inv_step;
        let v = match extremes.map(|e| e[bx]) {
            Some((u64::MAX, _)) => c.ceil(),
            Some((_, true)) => c.floor(),
            _ => c.round(),
        };
        let v = v.clamp(-CAP, CAP) as i32;
        q[(bx * NCH + ch) * 64] = v;
        let m = &mut nz[bx * NCH + ch];
        *m = (*m & !1) | u64::from(v != 0);
    }
}

/// Lab: encoder override of the DC step multipliers (1/64 units, luma then
/// chroma); 0 keeps the defaults. Written to the file, so it decodes anywhere.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_STEP_OVERRIDE: [core::sync::atomic::AtomicU8; 2] = [
    core::sync::atomic::AtomicU8::new(0),
    core::sync::atomic::AtomicU8::new(0),
];

/// Lab A/B switch for nearest and clamp-aware DC rounding (on by default).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_NEAREST: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

#[cfg(feature = "encode")]
pub(crate) fn dc_nearest(cfg: &LossyConfig) -> bool {
    #[cfg(feature = "lab")]
    {
        cfg.lab
            .dc_nearest
            .unwrap_or_else(|| DC_NEAREST.load(core::sync::atomic::Ordering::Relaxed))
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        true
    }
}

/// Lab: the DC step override of an encode ([`LossyLabSwitches::dc_step`]).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn lab_dc_step(cfg: &LossyConfig) -> [u8; 2] {
    cfg.lab.dc_step.unwrap_or_else(|| {
        DC_STEP_OVERRIDE
            .each_ref()
            .map(|s| s.load(core::sync::atomic::Ordering::Relaxed))
    })
}

#[derive(Clone, Copy)]
pub(crate) struct Geom {
    w: usize,
    h: usize,
    bw: usize,
    bh: usize,
    sb: usize,
}

impl Geom {
    fn new(w: u32, h: u32, sb: u32) -> Geom {
        Geom {
            w: w as usize,
            h: h as usize,
            bw: (w as usize).div_ceil(8),
            bh: (h as usize).div_ceil(8),
            sb: sb as usize,
        }
    }
    fn stripes(&self) -> usize {
        self.bh.div_ceil(self.sb)
    }
    /// Block rows [by0, by1) of stripe s.
    fn rows(&self, s: usize) -> (usize, usize) {
        let by0 = s * self.sb;
        (by0, (by0 + self.sb).min(self.bh))
    }
    fn active_lanes(&self) -> usize {
        self.bw.min(NL)
    }
}

/// Quantized coefficients of one stripe: `[(block * 3 + ch) * 64 + raster]`,
/// block = (by - by0) * bw + bx.
#[derive(Clone)]
#[cfg(feature = "encode")]
struct StripeCoefs {
    by0: usize,
    nby: usize,
    /// Blocks per row (the image's, or a column group's).
    bw: usize,
    q: Vec<i32>,
    /// Per (block, ch): bit k set when zigzag coefficient k is nonzero.
    nz: Vec<u64>,
    /// Per block: AQ code a (step multiplier a / 64); empty without AQ.
    aq: Vec<u8>,
    /// One three-bit transform selector per parent, empty for fixed layouts;
    /// with superblocks one merge role per parent.
    modes: Vec<u8>,
    /// Small transforms: per parent the bits of its 4x4 planes (bit c =
    /// plane c); empty without them.
    splits: Vec<u8>,
}

/// Channels coded together in one set of lane streams: channels
/// `ch0..ch0 + nch` of each block, laid out `[(block * nch + c) * 64]`. The
/// first channel's EOB / count context uses the lane's previous value of
/// that channel; the others use this block's first-channel value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub ch0: usize,
    pub nch: usize,
}

impl Group {
    /// All three channels in one group (4:4:4).
    pub(crate) const ALL: Group = Group { ch0: 0, nch: NCH };
}

/// The DC image carries the AQ channel: the activity rule (`aq_gamma`) or,
/// in lab builds, a caller-supplied map.
#[cfg(feature = "encode")]
fn aq_on_cfg(cfg: &LossyConfig) -> bool {
    cfg.aq_gamma != 0.0 || aq_map(cfg).is_some() || aq_field(cfg).is_some()
}

/// LAB: a lossy file's entropy-decoded coefficients (quantized DCs from the
/// nested DC image in slot 0) as (block width, block height, coefficients
/// at `(block * 3 + channel) * 64 + natural index`, AC record bytes, DC
/// record bytes), for coder comparisons on identical symbols. One column
/// group, default layouts.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn quantized_lab(data: &[u8]) -> Result<(usize, usize, Vec<i32>, usize, usize), Error> {
    if crate::parse::header::header_version(data)? == 4 {
        return v4::inspect_quantized(data);
    }
    let p = parse_lossy(data)?;
    if p.ngroups() != 1 || p.layouts != [LossyChannelLayout::default(); 3] {
        return Err(Error::Unsupported(
            "lab coefficient export: one column group and default layouts",
        ));
    }
    let g = &p.g;
    let mut out = vec![0i32; g.bw * g.bh * NCH * 64];
    for s in 0..g.stripes() {
        let (a, b) = g.rows(s);
        let (q, _, _, _) = decode_stripe_raw(&p, s, Group::ALL, false, Vec::new(), Vec::new())?;
        let n = (b - a) * g.bw * NCH * 64;
        out[a * g.bw * NCH * 64..][..n].copy_from_slice(&q[..n]);
    }
    let ac: usize = p.stripes.iter().map(|r| r.len()).sum();
    let dc_bytes: usize =
        p.dc.as_ref()
            .map_or(0, |d| d.stripes.iter().map(|r| r.len()).sum());
    let (aq, exact, bw, bh) = (p.aq, p.exact.is_some(), g.bw, g.bh);
    let (dinfo, px, _) = dc_image_lab(data)?;
    let (mut dv, mut scales) = (Vec::new(), Vec::new());
    dc_values(dinfo.sample, aq, exact, &px, &mut dv, &mut scales)?;
    for (blk, &d) in out.as_chunks_mut::<64>().0.iter_mut().zip(&dv) {
        blk[0] = d;
    }
    Ok((bw, bh, out, ac, dc_bytes))
}

/// LAB: per lossy unit (record order), per parent, the planes with AC (bit
/// c: a nonzero coefficient in slots 1..63): the planes that carry split
/// flags in a joint map (SPEC 2.11.1), for tests that forge maps.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn unit_ac_bits_lab(data: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    let p = parse_lossy(data)?;
    let ng = p.ngroups();
    let mut out = Vec::new();
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (_, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let (q, _, _, _) =
            decode_stripe_raw_bytes(&p, &gg, s, rec, Group::ALL, false, Vec::new(), Vec::new())?;
        out.push(superblock::ac_bits(&q, (b - a) * gg.bw));
    }
    Ok(out)
}

/// LAB: [`quantized_lab`] for files with partition maps too (the maps are
/// skipped; one column group).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn quantized_lab_any(data: &[u8]) -> Result<(usize, usize, Vec<i32>), Error> {
    let p = parse_lossy(data)?;
    let g = &p.g;
    let mut out = vec![0i32; g.bw * g.bh * NCH * 64];
    for (u, &bytes) in p.stripes.iter().enumerate() {
        let s = u / p.ngroups();
        let (x, gp) = p.group(u % p.ngroups());
        let (a, b) = gp.rows(s);
        let (q, _, _, _) =
            decode_stripe_raw_bytes(&p, &gp, s, bytes, Group::ALL, false, Vec::new(), Vec::new())?;
        let n = gp.bw * NCH * 64;
        for row in 0..b - a {
            let dst = ((a + row) * g.bw + x / 8) * NCH * 64;
            out[dst..dst + n].copy_from_slice(&q[row * n..][..n]);
        }
    }
    Ok((g.bw, g.bh, out))
}

/// LAB: superblock statistics of a lossy file: [parents, parents of 16x16,
/// 16x8 and 8x16 blocks, partition map bytes with their length varints].
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn superblock_stats_lab(data: &[u8]) -> Result<[u64; 5], Error> {
    let p = parse_lossy(data)?;
    let mut out = [0u64; 5];
    out[0] = (p.g.bw * p.g.bh) as u64;
    if p.sb.is_none() {
        return Ok(out);
    }
    let ng = p.ngroups();
    let mut roles = Vec::new();
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (_, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let (_, rest) = superblock::read_map(rec, b - a, gg.bw, &mut roles)?;
        out[4] += (rec.len() - rest.len()) as u64;
        for &r in &roles {
            if (1..=3).contains(&r) {
                out[r as usize] += 1;
            }
        }
    }
    Ok(out)
}

/// LAB: small-transform statistics of a lossy file: [parents, merged
/// parents, 4x4 planes of plane 0, 1 and 2, partition map bytes with their
/// length varints, AC record bytes] (column groups, exact and kept blocks
/// included).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn small_stats_lab(data: &[u8]) -> Result<[u64; 7], Error> {
    let p = parse_lossy(data)?;
    let mut out = [0u64; 7];
    out[0] = (p.g.bw * p.g.bh) as u64;
    out[6] = p.stripes.iter().map(|r| r.len() as u64).sum();
    if p.map == MapKind::None {
        return Ok(out);
    }
    let ng = p.ngroups();
    // Block multipliers (exact and kept blocks are bypassed).
    let scales: Vec<f32> = if p.aq {
        let (dinfo, px, _) = dc_image_lab(data)?;
        let (mut dv, mut sc) = (Vec::new(), Vec::new());
        dc_values(dinfo.sample, p.aq, p.exact.is_some(), &px, &mut dv, &mut sc)?;
        sc
    } else {
        Vec::new()
    };
    let skip = code0_skips(&p);
    let (mut roles, mut splits) = (Vec::new(), Vec::new());
    for (u, &rec) in p.stripes.iter().enumerate() {
        let (s, gi) = (u / ng, u % ng);
        let (x0, gg) = p.group(gi);
        let (a, b) = gg.rows(s);
        let rest = if p.map == MapKind::Joint {
            let (q, eobs, _, _) = decode_stripe_raw_bytes(
                &p,
                &gg,
                s,
                rec,
                Group::ALL,
                false,
                Vec::new(),
                Vec::new(),
            )?;
            let aq: Vec<f32> = if scales.is_empty() {
                Vec::new()
            } else {
                (a..b)
                    .flat_map(|by| {
                        let row = by * p.g.bw + x0 / 8;
                        scales[row..row + gg.bw].iter().copied()
                    })
                    .collect()
            };
            superblock::read_joint_map(
                rec,
                (b - a, gg.bw),
                (&aq, skip),
                (&q, &eobs),
                &mut roles,
                &mut splits,
            )?
            .2
        } else {
            splits.clear();
            superblock::read_map(rec, b - a, gg.bw, &mut roles)?.1
        };
        out[5] += (rec.len() - rest.len()) as u64;
        out[1] += roles.iter().filter(|&&r| r != 0).count() as u64;
        for &f in &splits {
            for c in 0..3 {
                out[2 + c] += (f >> c & 1) as u64;
            }
        }
    }
    Ok(out)
}

/// LAB: the nested DC image of a lossy file (its info, packed samples and
/// stripe height), for side-information studies and coefficient-parity tests.
#[cfg(any(test, feature = "lab"))]
#[cfg(feature = "encode")]
pub(crate) fn dc_image_lab(data: &[u8]) -> Result<(ImageInfo, Vec<u8>, u32), Error> {
    #[cfg(feature = "lab")]
    if crate::parse::header::header_version(data)? == 4 {
        return v4::inspect_dc_image(data);
    }
    let mut p = parse_lossy(data)?;
    let dc =
        p.dc.take()
            .ok_or(Error::Corrupt("lossy file without DC image"))?;
    let (dinfo, hs) = (dc.info.clone(), dc.hs as u32);
    let rb = dinfo.row_bytes();
    let mut out = vec![0u8; rb * dinfo.height as usize];
    if dc.unit_rectangles.is_some() || p.model_refs.is_some() {
        let ng = dc.ngroups();
        for (u, &bytes) in dc.stripes.iter().enumerate() {
            let (a, b, gi) = dc.unit_rectangles.as_ref().map_or_else(
                || {
                    let a = u / ng * dc.hs;
                    (a, (a + dc.hs).min(dc.info.height as usize), u % ng)
                },
                |v| v[u],
            );
            let (x, w, _) = dc.group(gi);
            let n = w * dc.bpp();
            let (model, body) = if let Some(models) = &p.model_refs {
                models.dc(bytes, &dc)?
            } else {
                (None, bytes)
            };
            let model = model.as_deref().unwrap_or(&dc);
            let om = crate::v1::OutMap::new(&model.info, &model.layout);
            let mut rows = crate::v1::UnitRows::new_with(model, u, body)?;
            for y in a..b {
                rows.next_row(model, &om, &mut out[y * rb + x * dc.bpp()..][..n])?;
            }
        }
        return Ok((dinfo, out, hs));
    }
    let mut stream = crate::v1::LosslessStream::from_parsed(dc);
    for row in out.chunks_mut(rb) {
        if !stream.next_row(row)? {
            return Err(Error::Corrupt("short DC image"));
        }
    }
    Ok((dinfo, out, hs))
}

#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn aq_map(cfg: &LossyConfig) -> Option<&[u8]> {
    cfg.aq_map.as_deref()
}

#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn aq_field(cfg: &LossyConfig) -> Option<LossyAqField> {
    cfg.aq_field
}

#[cfg(not(feature = "lab"))]
#[cfg(feature = "encode")]
fn aq_field(_: &LossyConfig) -> Option<()> {
    None
}

#[cfg(not(feature = "lab"))]
#[cfg(feature = "encode")]
fn aq_map(_: &LossyConfig) -> Option<&[u8]> {
    None
}

/// LAB: replaces low-frequency AC coefficients by their difference from a
/// prediction made from neighbouring blocks' (dequantized) DCs, in the
/// quantized domain, so a decoder could add it back exactly. Ratios from
/// linear / quadratic / bilinear ramps through the orthonormal DCT.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
fn lf_pred_lab(
    stripes: &mut [StripeCoefs],
    g: &Geom,
    steps: &[[f32; 64]; 3],
    mode: u8,
    dc_rows: usize,
) {
    let cm = dct::dct_matrix();
    let fdct = |f: &dyn Fn(f32, f32) -> f32, ox: f32, oy: f32| -> [f32; 64] {
        let mut out = [0.0f32; 64];
        for v in 0..8 {
            for u in 0..8 {
                let mut s = 0.0;
                for y in 0..8 {
                    for x in 0..8 {
                        s += cm[v][y] * cm[u][x] * f(ox + x as f32, oy + y as f32);
                    }
                }
                out[v * 8 + u] = s;
            }
        }
        out
    };
    let ramp = |x: f32, _: f32| x;
    let a01 = fdct(&ramp, 0.0, 0.0)[1] / (fdct(&ramp, 8.0, 0.0)[0] - fdct(&ramp, -8.0, 0.0)[0]);
    let quad = |x: f32, _: f32| (x - 3.5) * (x - 3.5);
    let b02 = fdct(&quad, 0.0, 0.0)[2]
        / (fdct(&quad, -8.0, 0.0)[0] + fdct(&quad, 8.0, 0.0)[0] - 2.0 * fdct(&quad, 0.0, 0.0)[0]);
    let bil = |x: f32, y: f32| (x - 3.5) * (y - 3.5);
    let c11 = fdct(&bil, 8.0, 8.0)[0] - fdct(&bil, -8.0, 8.0)[0] - fdct(&bil, 8.0, -8.0)[0]
        + fdct(&bil, -8.0, -8.0)[0];
    let g11 = fdct(&bil, 0.0, 0.0)[9] / c11;
    let (bw, bh) = (g.bw, g.bh);
    // Dequantized DC of every block and channel.
    let mut dc = vec![0.0f32; bw * bh * NCH];
    for sc in stripes.iter() {
        for b in 0..sc.nby * bw {
            for ch in 0..NCH {
                dc[(sc.by0 * bw + b) * NCH + ch] = sc.q[(b * NCH + ch) * 64] as f32 * steps[ch][0];
            }
        }
    }
    let local = mode & 4 != 0;
    let up_ok = |by: usize| by > 0 && !(local && by.is_multiple_of(dc_rows));
    let down_ok = |by: usize| by + 1 < bh && !(local && (by + 1).is_multiple_of(dc_rows));
    let zz_of = |k: usize| ZIGZAG.iter().position(|&z| z as usize == k).unwrap();
    let ks: &[usize] = if mode & 2 != 0 {
        &[1, 8, 2, 16, 9]
    } else {
        &[1, 8]
    };
    let gate = mode & 8 != 0;
    // Both one-sided slopes agree within 50% of the larger one.
    let smooth = |a: f32, c: f32, b: f32| {
        let (s1, s2) = (c - a, b - c);
        !gate || (s1 - s2).abs() <= 0.5 * s1.abs().max(s2.abs())
    };
    for sc in stripes.iter_mut() {
        for r in 0..sc.nby {
            let by = sc.by0 + r;
            for bx in 0..bw {
                let b = r * bw + bx;
                for ch in 0..NCH {
                    let at = |y: usize, x: usize| dc[(y * bw + x) * NCH + ch];
                    let c = at(by, bx);
                    let l = if bx > 0 { at(by, bx - 1) } else { c };
                    let rr = if bx + 1 < bw { at(by, bx + 1) } else { c };
                    let u = if up_ok(by) { at(by - 1, bx) } else { c };
                    let d = if down_ok(by) { at(by + 1, bx) } else { c };
                    let diag = up_ok(by) && down_ok(by) && bx > 0 && bx + 1 < bw;
                    for &k in ks {
                        let hs = smooth(l, c, rr);
                        let vs = smooth(u, c, d);
                        let p = match k {
                            1 if hs => a01 * (rr - l),
                            8 if vs => a01 * (d - u),
                            2 if hs => b02 * (l + rr - 2.0 * c),
                            16 if vs => b02 * (u + d - 2.0 * c),
                            1 | 8 | 2 | 16 => 0.0,
                            _ if diag && hs && vs => {
                                g11 * (at(by + 1, bx + 1) - at(by + 1, bx - 1) - at(by - 1, bx + 1)
                                    + at(by - 1, bx - 1))
                            }
                            _ => 0.0,
                        };
                        let pq = (p / steps[ch][k]).round() as i32;
                        let i = (b * NCH + ch) * 64 + k;
                        sc.q[i] -= pq;
                        let z = zz_of(k);
                        let m = &mut sc.nz[b * NCH + ch];
                        if sc.q[i] != 0 {
                            *m |= 1 << z;
                        } else {
                            *m &= !(1 << z);
                        }
                    }
                }
            }
        }
    }
}

/// Default lossy column group width in pixels; images wider than this get
/// groups. Zen 4, 12 threads: 43534x900 screenshots 2.3x, 32768x2048
/// photos +68%, 1 thread unchanged, +0.4..+1.0% bytes (4096-wide groups:
/// 2.6x / +70% for +0.9..+2.0% bytes and -2..-4% at 1 thread; each unit's
/// 16 lane states cost 64 bytes). benchmarks/lossy_column_groups_zen4_2026-10-03.md.
#[cfg(feature = "encode")]
const AUTO_GROUP_COLS: usize = 8192;

/// DC stripe height in block rows for lossy stripes `sb` block rows tall in
/// a `width` x `height` image: a multiple of the lossy stripe height (a DC
/// stripe covers whole lossy stripes), at least about 8 block rows, and
/// taller where the tier allows: `Balanced` (and no tier) keeps at least 32
/// DC stripes per image, `Compact` at least 8 (16 with a decoder memory
/// budget), as the largest power-of-two pixel height <= height / N, and one
/// for small images (`compact_one_dc_stripe`); `Fast`/`Fastest` stay at 8
/// block rows. Each DC boundary costs bytes, but a DC stripe is decoded
/// serially, so many-thread decode speed tracks the number of DC stripes
/// (benchmarks/lossy_stripe_decode_2026-10-07, lossy_dc_schedule_2026-10-07,
/// lossy_dc_limits_2026-10-07).
/// LAB: `set_lossy_dc_stripes` overrides the lossy stripes per DC stripe.
#[cfg(feature = "encode")]
pub(crate) fn dc_stripe_rows(cfg: &LossyConfig, sb: u32, width: u32, height: u32) -> usize {
    let sb = sb.max(1) as usize;
    #[cfg(feature = "lab")]
    match cfg
        .lab
        .dc_stripes
        .unwrap_or_else(|| DC_STRIPES_LAB.load(core::sync::atomic::Ordering::Relaxed))
    {
        0 => {}
        k => return (sb * k as usize).min(u32::MAX as usize),
    }
    let base = sb * (8 / sb).max(1);
    use crate::DecodeSpeed as D;
    let min_stripes = match cfg.decode_speed.unwrap_or(D::Balanced) {
        D::Compact if compact_one_dc_stripe(cfg, width, height) => {
            // Whole lossy stripes covering the image.
            return base.max((height as usize).div_ceil(8).div_ceil(sb) * sb);
        }
        // Without a memory budget at least 8 (benchmarks/lossy_dc_limits_2026-10-07).
        D::Compact if cfg.decoder_memory_limit.is_none() => 8,
        D::Compact => 16,
        D::Balanced => 32,
        D::Fast | D::Fastest => return base,
    };
    let target_px = (height as usize / min_stripes).max(1);
    // Largest power of two <= target, in block rows, whole lossy stripes.
    let rows = (1usize << target_px.ilog2()) / 8 / sb * sb;
    base.max(rows)
}

/// `Compact` codes images of up to `COMPACT_ONE_DC_PIXELS` with one DC
/// stripe. Each DC boundary costs bytes: one stripe instead of 2-4 saves
/// 3.2-4.3% at q5, 1.0-1.2% at q30 and 0.3-0.4% at q90 (128-256 tall).
/// One-thread decodes (`decode`), streaming and the first row stay within a
/// few percent up to 2048x2048, but with more threads one DC stripe is a
/// serial chain: at 256x256 up to -37% at 4-8 threads on M4 and N1 (at most
/// +37 / +156 us) and -20% on Zen 4; at 512x512 up to -50% (+152 / +465 us)
/// (benchmarks/lossy_dc_schedule_2026-10-07). A decoder memory budget keeps
/// the 16-stripe rule: one DC stripe raises the planner's estimate (2 x 64
/// bytes per block of a DC stripe per worker) far more than the decoder's
/// use.
#[cfg(feature = "encode")]
fn compact_one_dc_stripe(cfg: &LossyConfig, width: u32, height: u32) -> bool {
    cfg.decoder_memory_limit.is_none()
        && u64::from(width) * u64::from(height) <= COMPACT_ONE_DC_PIXELS
}

/// Largest image `Compact` codes with one DC stripe (256 x 256).
#[cfg(feature = "encode")]
const COMPACT_ONE_DC_PIXELS: u64 = 65_536;

/// LAB: lossy stripes per DC stripe (0 = automatic).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_STRIPES_LAB: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// LAB (coefgap study): the decode-cost proxy of EOB block-bucket contexts
/// (see `eob_bucket_proxy`).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static EOB_BKT_PROXY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// LAB: the proxy's mode (see `eob_bucket_proxy`), 1 by default.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static EOB_BKT_PROXY_MODE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(1);

/// LAB: an offset added to every proxy bucket (0; 1 only to check that the
/// proxy reaches the kernels: decodes then go wrong).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static EOB_BKT_PROBE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// LAB (coefgap study): the decode work of an EOB context split by a
/// per-block bucket (a candidate format change, not implemented): per stripe
/// the luma DC gradient |DC - left DC| + |DC - above DC| of every block,
/// quantized by three thresholds into an EOB context offset (bucket x 24),
/// which the run-level kernels load at each block start and add to the EOB
/// context. The entropy decode has no DC values yet, so this proxy runs the
/// same arithmetic on a zero buffer and scales the offsets by a runtime zero:
/// the coefficients and pixels stay exact. Not modelled: moving the stripe's
/// DC rows ahead of its entropy decode in multi-threaded schedules.
#[cfg(feature = "lab")]
fn eob_bucket_proxy(nb: usize, bw: usize) -> Option<alloc::vec::Vec<u32>> {
    use core::sync::atomic::Ordering::Relaxed;
    // Modes: 0 off, 1 bucket pass and kernel loads, 2 the pass only (the
    // kernels get no buckets), 3 the kernel loads only (a zero array).
    let mode = EOB_BKT_PROXY_MODE.load(Relaxed);
    if !EOB_BKT_PROXY.load(Relaxed) || mode == 0 {
        return None;
    }
    let mut out = alloc::vec![0u32; nb + NL];
    if mode != 3 {
        let dc = alloc::vec![0i32; nb];
        let zero = core::hint::black_box(0u32);
        let probe = EOB_BKT_PROBE.load(Relaxed);
        let th = [4i32, 12, 32];
        let bucket = |g: i32| -> u32 {
            ((g > th[0]) as u32 + (g > th[1]) as u32 + (g > th[2]) as u32) * 24 * zero + probe
        };
        for (r, (row, o)) in dc.chunks(bw).zip(out[..nb].chunks_mut(bw)).enumerate() {
            // Left neighbours in the row; the row above inside the stripe.
            o[0] = bucket(if r > 0 {
                (row[0] - dc[(r - 1) * bw]).abs()
            } else {
                0
            });
            if r > 0 {
                let up = &dc[(r - 1) * bw..r * bw];
                for (((v, &d), &l), &u) in o[1..].iter_mut().zip(&row[1..]).zip(row).zip(&up[1..]) {
                    *v = bucket((d - l).abs() + (d - u).abs());
                }
            } else {
                for ((v, &d), &l) in o[1..].iter_mut().zip(&row[1..]).zip(row) {
                    *v = bucket((d - l).abs());
                }
            }
        }
        if mode == 2 {
            core::hint::black_box(&out);
            return None;
        }
    }
    Some(out)
}

// Called only from the x86 two-stripe decode path; other targets never use it.
#[cfg(not(feature = "lab"))]
#[allow(dead_code)]
fn eob_bucket_proxy(_: usize, _: usize) -> Option<alloc::vec::Vec<u32>> {
    None
}

/// LAB (coefgap study): an override of how the lossy encoder picks its
/// table counts ([`TABLE_K_DEFAULT`]: the production rule,
/// `coef::k_search_share`): 0 the effort's budgets, 1 the budgets' clusters
/// merged down to the count that codes smallest (`coef::merge_by_cost`), 2 the
/// counts whose seeded tables code smallest (`coef::KSearch`; split-Huffman:
/// `huf::HSearch`), 3 mode 2 only where the tables weigh
/// (`coef::K_SEARCH_MIN_SHARE`; the production rule from effort 7.5).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static TABLE_K_SEARCH: core::sync::atomic::AtomicU8 =
    core::sync::atomic::AtomicU8::new(TABLE_K_DEFAULT);

/// LAB: [`TABLE_K_SEARCH`] without an override.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) const TABLE_K_DEFAULT: u8 = u8::MAX;

/// LAB (coefgap study): lanes of the nested DC image (0 = the lossless
/// encoder's default, `default_lanes`).
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) static DC_LANES_LAB: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// The lab table-count override of an encode
/// ([`LossyLabSwitches::table_k_mode`]; `u8::MAX`, none: the production rule,
/// always without `lab`).
#[cfg(feature = "encode")]
pub(crate) fn table_k_mode(cfg: &LossyConfig) -> u8 {
    #[cfg(feature = "lab")]
    {
        cfg.lab
            .table_k_mode
            .unwrap_or_else(|| TABLE_K_SEARCH.load(core::sync::atomic::Ordering::Relaxed))
    }
    #[cfg(not(feature = "lab"))]
    {
        let _ = cfg;
        u8::MAX
    }
}

/// LAB: the base tables of preset `id` (0..=[`MATRIX_LAST_PRESET`], raster
/// order, as the decoder reads them): imij-cmp's explicit-table catalog
/// builds its variants (swapped chroma and the like) from these.
#[cfg(feature = "lab")]
#[doc(hidden)]
#[cfg(feature = "encode")]
pub fn lab_base_tables(id: u8) -> Option<[[f32; 64]; 3]> {
    (id <= MATRIX_LAST_PRESET).then(|| {
        let qp = QParams::from_quality(90.0, LossyColor::YCoCg, id);
        [0, 1, 2].map(|c| qp.base(c))
    })
}

/// The nested DC image's lane count (`None` = the lossless default).
#[cfg(feature = "encode")]
fn dc_lanes(cfg: &LossyConfig) -> Option<u32> {
    #[cfg(feature = "lab")]
    {
        let n = cfg
            .lab
            .dc_lanes
            .unwrap_or_else(|| DC_LANES_LAB.load(core::sync::atomic::Ordering::Relaxed));
        if n > 0 {
            return Some(n);
        }
    }
    let _ = cfg;
    None
}

/// Lossy column group width for an image `w` wide (0 = none; else a
/// multiple of 8 below `w`).
#[cfg(feature = "encode")]
pub(crate) fn lossy_group_cols(cfg: &LossyConfig, w: usize) -> usize {
    let g = match cfg.column_groups {
        Some(g) => g as usize & !7,
        None if w > AUTO_GROUP_COLS => AUTO_GROUP_COLS,
        None => 0,
    };
    if g == 0 || g >= w { 0 } else { g }
}

/// Retain legacy packed-bank robustness/resource fixtures while the default
/// writer uses the general varint field. Entropy payloads are unchanged.
#[cfg(test)]
pub(crate) fn legacy_seed_fixture(file: &[u8]) -> Vec<u8> {
    let (info, color, body, declared) = crate::v1::lossy_body(file).unwrap().unwrap();
    let header = crate::v1::header(file).unwrap();
    let (_, meta, _) = crate::v1::parse_header(file).unwrap();
    let mut r = ByteReader::new(body);
    r.u8().unwrap();
    r.varint().unwrap();
    let prefix = r.pos;
    let mut e =
        envelope::read(&mut r, color, InputKind::of(&info).unwrap(), declared, true).unwrap();
    let rest = r.pos;
    if e.seed.is_some() {
        assert!(e.seed.bank <= 15);
        e.seed.legacy = true;
    }
    let mut data = body[..prefix].to_vec();
    envelope::write(&mut data, color, InputKind::of(&info).unwrap(), &e);
    data.extend_from_slice(&body[rest..]);
    let mut features = declared.without(crate::parse::header::Feature::SeedBankIds);
    if e.seed.is_some() {
        features = features.with(crate::parse::header::Feature::SeedBank1);
    }
    let mut out = Vec::new();
    crate::container::write_head_features(
        &mut out,
        &info,
        &meta,
        crate::Coding::Lossy,
        Some(color),
        header.decode_speed,
        features,
    )
    .unwrap();
    crate::container::put_data(&mut out, &data);
    out
}

/// Lab: entropy mode and the independent bank/scope selection.
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn lab_entropy_seed(
    file: &[u8],
) -> Result<(LossyEntropy, crate::seeds::Selection), Error> {
    if crate::parse::header::header_version(file)? == 4 {
        return v4::inspect_models(file).map(|(coder, seed, _)| (coder, seed));
    }
    let (info, color, body, declared) =
        crate::v1::lossy_body(file)?.ok_or(Error::InvalidInput("not a lossy file"))?;
    let mut r = ByteReader::new(body);
    r.u8()?; // stripe rows
    r.varint()?; // column groups
    let kind = InputKind::of(&info).ok_or(Error::Corrupt("pixel format"))?;
    let e = envelope::read(&mut r, color, kind, declared, true)?;
    Ok((e.entropy, e.seed))
}

/// Lab: the entropy mode and AC entropy-table payload bytes of a lossy
/// file: the length-prefixed table section between the parameter envelope
/// and the head records / nested heads (what `Tables::write` /
/// `huf::HufTables::write` produced). Reads the same grammar as
/// [`lab_entropy_seed`].
#[cfg(feature = "lab")]
#[cfg(feature = "encode")]
pub(crate) fn lab_ac_table_bytes(file: &[u8]) -> Result<(LossyEntropy, u64), Error> {
    if crate::parse::header::header_version(file)? == 4 {
        return v4::inspect_models(file).map(|(coder, _, bytes)| (coder, bytes));
    }
    let (info, color, body, declared) =
        crate::v1::lossy_body(file)?.ok_or(Error::InvalidInput("not a lossy file"))?;
    let mut r = ByteReader::new(body);
    r.u8()?; // stripe rows
    r.varint()?; // column groups
    let kind = InputKind::of(&info).ok_or(Error::Corrupt("pixel format"))?;
    let e = envelope::read(&mut r, color, kind, declared, true)?;
    Ok((e.entropy, r.varint()?))
}

/// Native component coefficient carrier, independent of source JPEG scans.
/// Coded dimensions include the source's retained partial/MCU blocks.
#[cfg(feature = "encode")]
#[cfg(feature = "native-jpeg")]
pub(crate) fn encode_native_component(
    width: u32,
    height: u32,
    table: &[u16; 64],
    coefficients: &[i16],
    entropy: LossyEntropy,
    effort: f32,
) -> Result<Vec<u8>, Error> {
    let info = ImageInfo::new(width, height, Channels::Gray, SampleType::U8);
    info.validate_precision()?;
    if width == 0
        || height == 0
        || !width.is_multiple_of(8)
        || !height.is_multiple_of(8)
        || table.contains(&0)
    {
        return Err(Error::InvalidInput(
            "native component coded grid or quantizer",
        ));
    }
    let cfg = LossyConfig {
        color: LossyColor::YCoCg,
        quality: 50.0,
        entropy,
        effort,
        channel_mul: Some([1.0; 3]),
        column_groups: Some(0),
        exact_blocks: Some(0.0),
        ..LossyConfig::default()
    };
    let sb = cfg.stripe_rows_for(width);
    let g = Geom::new(width, height, sb);
    let count =
        g.bw.checked_mul(g.bh)
            .and_then(|n| n.checked_mul(64))
            .ok_or(Error::TooLarge)?;
    if count != coefficients.len() {
        return Err(Error::InvalidInput("native component coefficient count"));
    }
    let mut q = Vec::new();
    resize_fallible(&mut q, count.checked_mul(NCH).ok_or(Error::TooLarge)?, 0i32)?;
    for (src, dst) in coefficients
        .as_chunks::<64>()
        .0
        .iter()
        .zip(q.as_chunks_mut::<{ NCH * 64 }>().0.iter_mut())
    {
        for (a, &b) in dst[..64].iter_mut().zip(src) {
            *a = i32::from(b);
        }
    }
    let mut qp = QParams::from_quality(50.0, LossyColor::YCoCg, MATRIX_EXPLICIT);
    qp.mul = [1.0; 3];
    qp.dc = [64; 3];
    qp.explicit = Some([table.map(f32::from); 3]);
    if !qp.valid() {
        return Err(Error::InvalidInput("native quantization table"));
    }
    let file = finish_quantized_coefficients(
        &info,
        &Metadata::default(),
        &cfg,
        crate::effort::Effort::new(effort)?,
        &g,
        sb,
        qp,
        &q,
    )?;
    let file = crate::container::finish_unit_table(file)?;
    crate::container::locate(file)
}

/// A strict coefficient carrier reader. It shares production stripe/DC
/// decoding and refuses any declaration that would edit native spectra.
#[cfg(feature = "native-jpeg")]
pub(crate) fn decode_native_component(
    file: &[u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
) -> Result<Vec<i16>, Error> {
    decode_native_component_checked(
        file,
        width,
        height,
        table,
        &crate::parse::limits::Unrestricted,
    )
}

#[cfg(feature = "native-jpeg")]
pub(crate) fn parse_native_component_checked<'a>(
    file: &'a [u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    let mut raw = ByteReader::new(file);
    let hp = crate::parse::header::read_header_sized(&mut raw)?;
    crate::parse::chunks::walk_to_data(&mut raw, |_, _| {})?;
    if crate::parse::payload::native_selected(hp.header.lossy_color, hp.declared, &file[raw.pos..])?
    {
        return Err(Error::Corrupt("nested native component envelope"));
    }
    let f = crate::v1::parse_file(file, admission, crate::checksum::Verify::Now(1))?;
    if f.header.info.width != width
        || f.header.info.height != height
        || f.header.info.channels != Channels::Gray
        || f.header.info.sample != SampleType::U8
        || f.header.info.has_custom_precision()
        || f.header.lossy_color != Some(LossyColor::YCoCg)
        || width == 0
        || height == 0
        || !width.is_multiple_of(8)
        || !height.is_multiple_of(8)
        || table.contains(&0)
    {
        return Err(Error::Corrupt("native component carrier header"));
    }
    let p = parse_lossy_checked(
        f.header.info.clone(),
        f.header.version,
        LossyColor::YCoCg,
        f.declared,
        f.data,
        &crate::parse::limits::Checked(admission),
    )?;
    let expected = dct::dequant_factors(&table.map(f32::from));
    if !p.native_quantizer
        || p.ngroups() != 1
        || p.aq
        || p.skip
        || p.exact.is_some()
        || p.sb.is_some()
        || p.alpha.is_some()
        || p.bias.is_some()
        || p.transform.is_some()
        || p.layouts != [LossyChannelLayout::default(); 3]
        || p.inv_scale != [1.0; 3]
        || p.dq != [expected; 3]
    {
        return Err(Error::Corrupt(
            "native component carrier changes coefficients",
        ));
    }
    Ok(p)
}

#[cfg(feature = "native-jpeg")]
#[cfg(feature = "native-jpeg")]
pub(crate) fn decode_native_component_checked(
    file: &[u8],
    width: u32,
    height: u32,
    table: &[u16; 64],
    admission: &impl crate::parse::limits::Admission,
) -> Result<Vec<i16>, Error> {
    let mut p = parse_native_component_checked(file, width, height, table, admission)?;
    let dc =
        p.dc.take()
            .ok_or(Error::Corrupt("native component DC image"))?;
    let dinfo = dc.info.clone();
    let dc_stride = dinfo.row_bytes();
    let mut dc_rows = Vec::new();
    resize_fallible(
        &mut dc_rows,
        dc_stride.checked_mul(p.g.sb).ok_or(Error::TooLarge)?,
        0u8,
    )?;
    let mut stream = crate::v1::LosslessStream::from_parsed(dc);
    let mut out = Vec::new();
    let count =
        p.g.bw
            .checked_mul(p.g.bh)
            .and_then(|n| n.checked_mul(64))
            .ok_or(Error::TooLarge)?;
    out.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
    let (mut q, mut eobs, mut values, mut scales) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for stripe in 0..p.g.stripes() {
        crate::parse::limits::Admission::check(admission)?;
        let (a, b) = p.g.rows(stripe);
        let rows = b - a;
        for y in 0..rows {
            admission.check()?;
            if !stream.next_row(&mut dc_rows[y * dc_stride..(y + 1) * dc_stride])? {
                return Err(Error::Corrupt("short native component DC image"));
            }
        }
        dc_values(
            dinfo.sample,
            false,
            false,
            &dc_rows[..rows * dc_stride],
            &mut values,
            &mut scales,
        )?;
        let raw = decode_stripe_raw_bytes(
            &p,
            &p.g,
            stripe,
            p.stripes[stripe],
            Group::ALL,
            true,
            core::mem::take(&mut q),
            core::mem::take(&mut eobs),
        )?;
        finish_coefs(&p, &p.g, stripe, &values, raw, &mut q, &mut eobs)?;
        for block in q[..rows * p.g.bw * NCH * 64].as_chunks::<{ NCH * 64 }>().0 {
            if block[64..].iter().any(|&v| v != 0) {
                return Err(Error::Corrupt("nonzero unused native component channel"));
            }
            for &v in &block[..64] {
                out.push(i16::try_from(v).map_err(|_| Error::Corrupt("native coefficient range"))?);
            }
        }
        q.fill(0);
        values.clear();
        scales.clear();
    }
    let mut final_row = Vec::new();
    resize_fallible(&mut final_row, dc_stride, 0u8)?;
    if stream.next_row(&mut final_row)? {
        return Err(Error::Corrupt("long native component DC image"));
    }
    Ok(out)
}

#[cfg(feature = "lab")]
#[cfg(feature = "native-jpeg")]
pub(crate) fn jpeg_idct_lab(q: &[i16; 64], table: &[u16; 64]) -> [f32; 64] {
    jpeg::idct(q, table)
}

#[cfg(feature = "lab")]
pub(crate) fn linear_rgb_lab(matrix: &[[f32; 4]; 3], values: [f32; 3]) -> [u8; 3] {
    color::inverse_linear_px(matrix, [1.0; 3], values)
}

mod decode;
#[cfg(feature = "lab")]
pub(crate) use decode::IDCT_FMA;
#[allow(unused_imports)]
#[cfg(any(feature = "v3", feature = "lab"))]
pub(crate) use decode::RegionGeometry;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
pub(crate) use decode::aq_dequant_in;
#[cfg(all(target_arch = "x86_64", any(test, feature = "lab")))]
pub(crate) use decode::force_gather;
#[cfg(all(target_arch = "x86_64", feature = "lab"))]
pub(crate) use decode::kernel_choice_str;
use decode::parse::RansTables;
#[cfg(feature = "lab")]
use decode::parse::trace_lossy_group;
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) use decode::parse_lossy;
#[allow(unused_imports)]
pub(crate) use decode::parse_lossy_head_checked;
#[cfg(all(test, feature = "encode"))]
use decode::recon::decode_lossy_impl;
#[cfg(feature = "lab")]
use decode::recon::decode_stripe_coefs;
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
use decode::recon::decode_stripe_raw;
#[cfg(feature = "native-jpeg")]
use decode::recon::finish_coefs;
use decode::recon::{
    AcModelView, RECON_CHUNK, code0_skips, color_chunk_row, dc_values, decode_coefficient_unit,
    decode_stripe_bytes_mode, decode_stripe_raw_bytes, fill_dc, flat_run, pack_rows,
    recon_chunk_blocks, recon_stripe_mode, resize_fallible,
};
#[cfg(all(test, feature = "std", feature = "encode"))]
use decode::recon::{SHARED_DC, SHARED_DC_USED};
#[cfg(all(test, feature = "encode", feature = "lab", feature = "std"))]
pub(crate) use decode::scoped_recon_chunk_override;
#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
pub(crate) use decode::set_shared_dc;
pub(crate) use decode::{
    LossyParsed, MapKind, Scratch, aq_dequant, decode_lossy_frame, decode_lossy_parsed,
    decode_lossy_parsed_checked, decode_lossy_parsed_observed, decoder_recon_bytes,
    decoder_table_bytes, decoder_transfer_table_bytes, parse_lossy_as, parse_lossy_checked,
    parse_lossy_head_async, payload_len_checked, prof,
};
#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) use decode::{decode_lossy_frame_observed, set_keep_scratch};
#[cfg(feature = "v3")]
pub(crate) use decode::{decode_lossy_region, region_unit_bytes};
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) use decode::{set_dc_split, set_pairs, set_recon_chunk};
#[allow(unused_imports)]
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(crate) use decode::{split_group, split_unpack_bc};

#[cfg(all(test, feature = "encode"))]
pub(crate) mod tests;

#[cfg(all(test, feature = "lab", feature = "std", not(target_family = "wasm")))]
mod overlap_study;

#[cfg(all(test, feature = "encode"))]
mod color_precision_tests;

#[cfg(all(test, feature = "encode"))]
mod hdr_precision_lab_tests;

#[cfg(all(test, feature = "encode"))]
mod pack_tests;

#[cfg(all(test, feature = "std", feature = "encode"))]
mod bundle_tests;

mod precision;
