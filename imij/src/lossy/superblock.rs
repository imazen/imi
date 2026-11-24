//! Superblocks (feature `superblocks`, Compact 0.2; SPEC 2.9): 16x16, 16x8
//! and 8x16 DCTs merged from aligned 8x8 parents of one unit.
//!
//! * **Geometry.** A merged block covers 2x2 parents (16x16), two parents
//!   side by side (16x8: 16 wide, 8 tall) or two stacked (8x16). Its
//!   top-left parent sits at an even unit-local block row and column, so a
//!   merge never crosses a stripe or a column group and every unit stays
//!   independently decodable.
//! * **Partition map.** Each lossy unit record starts with its map: one
//!   symbol per aligned 2x2 parent group, range coded with neighbour
//!   contexts ([`map`]).
//! * **Coefficients.** The orthonormal 2D DCT of the merged block. Its
//!   lowest (H/8) x (W/8) coefficients travel in the nested DC image as
//!   per-parent DCs (JPEG XL's scheme): along a 16-point axis the two lowest
//!   coefficients map to the two parents through the 2-point DCT, the second
//!   scaled by [`LLF_R`], so piecewise-constant content gives each parent its
//!   own block mean. The other coefficients fill the parents' 64-slot units as
//!   spectrum quadrants: unit p = (py, px) holds coefficients (8 py + i,
//!   8 px + j) at raster slot 8 i + j, except that the quadrant origins of
//!   units p > 0 (whose slot 0 carries parent p's DC) move to unit 0's raster
//!   slot 8 py + px. Entropy coding, contexts and lanes are unchanged.
//! * **Quantization.** Each coefficient uses the 8x8 step of its physical
//!   frequency (bilinear in the 8x8 step table), so one step costs the same
//!   error energy in every transform size. One AQ code per merged block.
//! * **Inverse.** Separable, columns then rows, each pass a fixed f32
//!   flowgraph without fused multiply-adds ([`kernel`]): the 8-point AAN
//!   pass of the 8x8 transform, and a 16-point pass whose even half is that
//!   AAN pass and whose odd half is an inverse DCT-IV. The scalar reference
//!   [`kernel::inverse_ref`] is the normative form; every tier computes the
//!   same operations in the same order, lane-wise.
//!
//! Design, selection and measurements: `docs/LOSSY.md` "Superblocks" and
//! `benchmarks/superblocks_2026-10-09`, `benchmarks/sbport_2026-10-10`.

use super::*;

pub(super) mod kernel;
pub(super) mod map;
#[cfg(feature = "encode")]
pub(super) mod select;

/// Lossy head record id of superblocks (critical, empty payload; feature
/// `superblocks`).
pub(super) const H_SUPERBLOCKS: u64 = crate::registry::HEAD_SUPERBLOCKS;

/// Merge roles in a unit's expanded map (one byte per parent).
pub(super) const ROLE_16X16: u8 = 1;
pub(super) const ROLE_16X8: u8 = 2;
pub(super) const ROLE_8X16: u8 = 3;

/// Merged shapes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Shape {
    S16x16 = 0,
    /// 16 wide, 8 tall: two parents side by side.
    S16x8 = 1,
    /// 8 wide, 16 tall: two stacked parents.
    S8x16 = 2,
}

impl Shape {
    pub(super) const ALL: [Shape; 3] = [Shape::S16x16, Shape::S16x8, Shape::S8x16];
    /// Width in pixels.
    pub(super) const fn w(self) -> usize {
        match self {
            Shape::S8x16 => 8,
            _ => 16,
        }
    }
    /// Height in pixels.
    pub(super) const fn h(self) -> usize {
        match self {
            Shape::S16x8 => 8,
            _ => 16,
        }
    }
    /// Parents across.
    pub(super) const fn pw(self) -> usize {
        self.w() / 8
    }
    /// Parents down.
    pub(super) const fn ph(self) -> usize {
        self.h() / 8
    }
    #[cfg(feature = "encode")]
    pub(super) const fn role(self) -> u8 {
        match self {
            Shape::S16x16 => ROLE_16X16,
            Shape::S16x8 => ROLE_16X8,
            Shape::S8x16 => ROLE_8X16,
        }
    }
    pub(super) const fn from_role(r: u8) -> Option<Shape> {
        match r {
            ROLE_16X16 => Some(Shape::S16x16),
            ROLE_16X8 => Some(Shape::S16x8),
            ROLE_8X16 => Some(Shape::S8x16),
            _ => None,
        }
    }
}

/// Roles of an aligned 2x2 parent group's top-left, top-right, bottom-left
/// and bottom-right parents per partition symbol: no merge, 16x16, top
/// 16x8, bottom 16x8, both 16x8, left 8x16, right 8x16, both 8x16.
pub(super) const PARTITIONS: [[u8; 4]; 8] = [
    [0, 0, 0, 0],
    [ROLE_16X16; 4],
    [ROLE_16X8, ROLE_16X8, 0, 0],
    [0, 0, ROLE_16X8, ROLE_16X8],
    [ROLE_16X8; 4],
    [ROLE_8X16, 0, ROLE_8X16, 0],
    [0, ROLE_8X16, 0, ROLE_8X16],
    [ROLE_8X16; 4],
];

/// The merged blocks of each partition symbol: (shape, parent row, parent
/// column) of each block's top-left parent within the group.
#[cfg(feature = "encode")]
pub(super) const BLOCKS: [&[(Shape, usize, usize)]; 8] = [
    &[],
    &[(Shape::S16x16, 0, 0)],
    &[(Shape::S16x8, 0, 0)],
    &[(Shape::S16x8, 1, 0)],
    &[(Shape::S16x8, 0, 0), (Shape::S16x8, 1, 0)],
    &[(Shape::S8x16, 0, 0)],
    &[(Shape::S8x16, 0, 1)],
    &[(Shape::S8x16, 0, 0), (Shape::S8x16, 0, 1)],
];

/// The partition symbol of a group's roles (`PARTITIONS` index), `None`
/// for roles no symbol has.
pub(super) fn symbol_of(roles: [u8; 4]) -> Option<u8> {
    PARTITIONS.iter().position(|p| *p == roles).map(|s| s as u8)
}

/// Aligned parent groups of a `rows` x `cols` unit in raster order: (row,
/// column, present parents in `PARTITIONS` order). A group whose only
/// parent is its top-left one (the bottom-right corner of a unit with an
/// odd number of rows and columns) carries no symbol and is not listed.
pub(super) fn map_groups(
    rows: usize,
    cols: usize,
) -> impl Iterator<Item = (usize, usize, [bool; 4])> {
    (0..rows).step_by(2).flat_map(move |r| {
        (0..cols).step_by(2).filter_map(move |x| {
            let (dy, dx) = (r + 1 < rows, x + 1 < cols);
            (dy || dx).then_some((r, x, [true, dx, dy, dx && dy]))
        })
    })
}

/// Unit and raster slot of coefficient (v, u) of a merged spectrum outside
/// its LLF corner: quadrant (v / 8, u / 8) at slot 8 (v % 8) + u % 8, its
/// origin (slot 0, the parent's DC) moved to unit 0's slot 8 py + px.
#[cfg(feature = "encode")]
pub(super) fn slot(s: Shape, v: usize, u: usize) -> (usize, usize) {
    let (py, px) = (v / 8, u / 8);
    let z = (v % 8) * 8 + u % 8;
    if z == 0 {
        (0, py * 8 + px)
    } else {
        (py * s.pw() + px, z)
    }
}

/// `1 / sqrt(2)`, rounded to f32: the 2-point DCT's factor.
pub(super) const K2: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// Scale of the second LLF coefficient of a 16-point axis: the 16-point
/// coefficient 1 of a signal constant on each half, over the 2-point DCT
/// coefficient 1 of the halves' 8-point DCs, `1 / (8 sqrt 2 sin(pi / 32))`
/// rounded to f32 (`llf_r_is_the_rounded_exact_ratio`).
pub(super) const LLF_R: f32 = f32::from_bits(0x3f66_da05);

/// Kernel input scale of frequency k of a 16-point axis: even k = 2m takes
/// `AAN[m] / 4` (the AAN pass of the even half, descaled by the split's
/// extra `1 / sqrt 2` and the 16-point basis), odd k takes `1 / (2 sqrt 2)`
/// (the odd half's basis scale). Feeding coefficient k multiplied by
/// `SC16[k]` makes the flowgraph compute the orthonormal inverse.
pub(super) const SC16: [f32; 16] = {
    let mut s = [0.0f32; 16];
    let mut m = 0;
    while m < 8 {
        s[2 * m] = dct::AAN[m] * 0.25;
        s[2 * m + 1] = K2 * 0.5;
        m += 1;
    }
    s
};

/// Kernel input scale of frequency k of an 8-point axis: `AAN[k] / sqrt 8`
/// (the per-axis factor of the 8x8 dequantization).
pub(super) const SC8: [f32; 8] = {
    let mut s = [0.0f32; 8];
    let mut k = 0;
    while k < 8 {
        s[k] = dct::AAN[k] * (K2 * 0.5);
        k += 1;
    }
    s
};

/// The kernel input scale of coefficient (v, u) of shape `s`: `SV[v] *
/// SH[u]`, one f32 multiply.
pub(super) fn input_scale(s: Shape, v: usize, u: usize) -> f32 {
    let sv = if s.h() == 16 { SC16[v] } else { SC8[v] };
    let su = if s.w() == 16 { SC16[u] } else { SC8[u] };
    sv * su
}

/// The step of coefficient (v, u) of shape `s` from a plane's 8x8 step
/// table `t` (raster, row = vertical frequency): the table bilinearly
/// interpolated at the coefficient's physical frequency (v 8 / H, u 8 / W).
/// Along a 16-point axis an odd index falls halfway between two entries
/// (the last one repeated past index 7), so the value is `t[a] * 0.5 +
/// t[b] * 0.5` per axis, vertical last; indices on entries take the entry.
pub(super) fn step(t: &[f32; 64], s: Shape, v: usize, u: usize) -> f32 {
    // Per axis: the two entries and whether the index falls between them.
    let axis = |k: usize, n: usize| -> (usize, usize, bool) {
        if n == 16 {
            ((k / 2), (k / 2 + 1).min(7), k % 2 == 1)
        } else {
            (k, k, false)
        }
    };
    let (y0, y1, my) = axis(v, s.h());
    let (x0, x1, mx) = axis(u, s.w());
    let row = |y: usize| -> f32 {
        if mx {
            t[y * 8 + x0] * 0.5 + t[y * 8 + x1] * 0.5
        } else {
            t[y * 8 + x0]
        }
    };
    if my {
        row(y0) * 0.5 + row(y1) * 0.5
    } else {
        row(y0)
    }
}

/// Per-image decode tables (from the quantizer steps).
pub(crate) struct Tables {
    /// Per plane, per shape: the dequantization factor of coefficient (v,
    /// u) at `v * 16 + u`, `step * input_scale` (one f32 multiply).
    pub(super) dq: [[[f32; 256]; 3]; NCH],
    /// Per shape: the input scale of every coefficient (the LLF corner's
    /// scale; the rest are folded into `dq`).
    pub(super) sc: [[f32; 256]; 3],
    /// Per plane: the DC step (the step of the parents' DCs).
    pub(super) dc_step: [f32; NCH],
    /// Per run-level EOB e: rows and columns spanned by zigzag positions
    /// 1..=e of a unit (0 for e = 0); bounds the nonzero spectrum.
    pub(super) zz_ext: [(u8, u8); 64],
}

impl Tables {
    pub(super) fn new(steps: &[[f32; 64]; NCH]) -> Self {
        let sc = Shape::ALL.map(|s| {
            let mut t = [0.0f32; 256];
            for v in 0..s.h() {
                for u in 0..s.w() {
                    t[v * 16 + u] = input_scale(s, v, u);
                }
            }
            t
        });
        let dq = core::array::from_fn(|c| {
            Shape::ALL.map(|s| {
                let mut t = [0.0f32; 256];
                for v in 0..s.h() {
                    for u in 0..s.w() {
                        t[v * 16 + u] = step(&steps[c], s, v, u) * sc[s as usize][v * 16 + u];
                    }
                }
                t
            })
        });
        let mut zz_ext = [(0u8, 0u8); 64];
        let (mut rows, mut cols) = (0u8, 0u8);
        for e in 1..64 {
            let z = ZIGZAG[e];
            rows = rows.max(z / 8 + 1);
            cols = cols.max(z % 8 + 1);
            zz_ext[e] = (rows, cols);
        }
        Self {
            dq,
            sc,
            dc_step: steps.map(|s| s[0]),
            zz_ext,
        }
    }
}

/// 2-point orthonormal DCT (its own inverse): `((a + b) K2, (a - b) K2)`.
#[inline]
pub(super) fn dct2(a: f32, b: f32) -> (f32, f32) {
    ((a + b) * K2, (a - b) * K2)
}

/// The LLF coefficients (row-major, `ph` x `pw`) of a merged block from its
/// parents' dequantized DCs: per column the 2-point DCT down a 16-point
/// axis, its second output times [`LLF_R`]; then per row across.
pub(super) fn dcs_to_llf(s: Shape, dcs: [[f32; 2]; 2]) -> [[f32; 2]; 2] {
    let mut l = dcs;
    if s.ph() == 2 {
        for x in 0..s.pw() {
            let (a, b) = dct2(l[0][x], l[1][x]);
            l[0][x] = a;
            l[1][x] = b * LLF_R;
        }
    }
    if s.pw() == 2 {
        for row in l.iter_mut().take(s.ph()) {
            let (a, b) = dct2(row[0], row[1]);
            *row = [a, b * LLF_R];
        }
    }
    l
}

/// The parents' DCs of a merged block from its LLF coefficients (encoder;
/// the inverse of [`dcs_to_llf`]).
#[cfg(feature = "encode")]
pub(super) fn llf_to_dcs(s: Shape, llf: [[f32; 2]; 2]) -> [[f32; 2]; 2] {
    let mut d = llf;
    if s.pw() == 2 {
        for row in d.iter_mut().take(s.ph()) {
            let (a, b) = dct2(row[0], row[1] / LLF_R);
            *row = [a, b];
        }
    }
    if s.ph() == 2 {
        for x in 0..s.pw() {
            let (a, b) = dct2(d[0][x], d[1][x] / LLF_R);
            d[0][x] = a;
            d[1][x] = b;
        }
    }
    d
}

// ---- decoding -----------------------------------------------------------

/// A unit record's bytes after its partition map (`varint len`, then `len`
/// bytes), for decoders that only entropy decode.
pub(super) fn skip_map(bytes: &[u8]) -> Result<&[u8], Error> {
    let mut r = ByteReader::new(bytes);
    let n = r.var_usize()?;
    r.bytes(n)?;
    Ok(&bytes[r.pos..])
}

/// Reads a unit record's partition map into `roles` (one merge role per
/// parent of the `rows` x `cols` unit) and returns whether anything merges
/// (`roles` is left empty otherwise) and the record's rest.
pub(super) fn read_map<'a>(
    bytes: &'a [u8],
    rows: usize,
    cols: usize,
    roles: &mut Vec<u8>,
) -> Result<(bool, &'a [u8]), Error> {
    let mut r = ByteReader::new(bytes);
    let n = r.var_usize()?;
    let body = r.bytes(n)?;
    roles.clear();
    if n == 0 {
        return Ok((false, &bytes[r.pos..]));
    }
    let parents = rows.checked_mul(cols).ok_or(Error::TooLarge)?;
    roles.try_reserve(parents).map_err(|_| Error::TooLarge)?;
    roles.resize(parents, 0);
    map::decode(body, rows, cols, roles)?;
    Ok((true, &bytes[r.pos..]))
}

/// Reads a unit record's joint map (lossy head record 23, feature
/// `small_transforms`, SPEC 2.11.1) into `roles` (one merge role per parent
/// of the `rows` x `cols` unit) and `splits` (one set of 4x4 plane bits per
/// parent); both are left empty when the map is (nothing merges or
/// splits). `aq`: the unit's block multipliers (empty without the DC
/// image's fourth channel); with `skip`, multiplier 0 marks bypassed
/// parents (exact or kept), which carry no flags. `q`: the unit's decoded
/// coefficients (`[(block * 3 + plane) * 64 + slot]`) and `eobs` their
/// per-plane counts: a plane carries a flag only when one of its slots
/// 1..63 is nonzero. Returns (anything merges, anything splits, the
/// record's rest).
pub(super) fn read_joint_map<'a>(
    bytes: &'a [u8],
    (rows, cols): (usize, usize),
    (aq, skip): (&[f32], bool),
    (q, eobs): (&[i32], &[u8]),
    roles: &mut Vec<u8>,
    splits: &mut Vec<u8>,
) -> Result<(bool, bool, &'a [u8]), Error> {
    let mut r = ByteReader::new(bytes);
    let n = r.var_usize()?;
    let body = r.bytes(n)?;
    roles.clear();
    splits.clear();
    if n == 0 {
        return Ok((false, false, &bytes[r.pos..]));
    }
    let parents = rows.checked_mul(cols).ok_or(Error::TooLarge)?;
    for v in [&mut *roles, &mut *splits] {
        v.try_reserve(parents).map_err(|_| Error::TooLarge)?;
        v.resize(parents, 0);
    }
    let bypass = |b: usize| skip && !aq.is_empty() && aq[b] == 0.0;
    // A plane has AC when a slot 1..63 is nonzero (its count says where
    // to look; a count over zeros has none).
    let mut ac = Vec::new();
    ac.try_reserve(parents).map_err(|_| Error::TooLarge)?;
    ac.extend((0..parents).map(|b| {
        (0..NCH).fold(0u8, |m, c| {
            let has = eobs[b * NCH + c] != 0
                && q[(b * NCH + c) * 64 + 1..(b * NCH + c + 1) * 64]
                    .iter()
                    .any(|&v| v != 0);
            m | (has as u8) << c
        })
    }));
    let merged = map::decode_joint(body, (rows, cols), roles, splits, &ac, &bypass)?;
    let split = splits.iter().any(|&f| f != 0);
    Ok((merged, split, &bytes[r.pos..]))
}

/// The per-parent AC bits of a unit for its joint map (encoder): bit c when
/// plane c has a nonzero coefficient in slots 1..63.
#[cfg(feature = "encode")]
pub(super) fn ac_bits(q: &[i32], parents: usize) -> Vec<u8> {
    (0..parents)
        .map(|b| {
            (0..NCH).fold(0u8, |m, c| {
                let u = &q[(b * NCH + c) * 64..][..64];
                m | (u[1..].iter().any(|&v| v != 0) as u8) << c
            })
        })
        .collect()
}

/// Checks a unit's merges against its block codes (`aq`: one multiplier per
/// parent, empty without the DC image's fourth channel): every parent of a
/// merged block has the same multiplier, and none is an exact or kept block
/// (`skip`: multiplier 0 marks those).
pub(super) fn validate(roles: &[u8], aq: &[f32], cols: usize, skip: bool) -> Result<(), Error> {
    if aq.is_empty() {
        return Ok(());
    }
    for (b, &role) in roles.iter().enumerate() {
        let Some(s) = Shape::from_role(role) else {
            continue;
        };
        let (r, x) = (b / cols, b % cols);
        if r % s.ph() != 0 || x % s.pw() != 0 {
            continue;
        }
        let a = aq[b];
        if skip && a == 0.0 {
            return Err(Error::Corrupt("superblock over an exact or kept block"));
        }
        for py in 0..s.ph() {
            for px in 0..s.pw() {
                if aq[(r + py) * cols + x + px].to_bits() != a.to_bits() {
                    return Err(Error::Corrupt(
                        "superblock parents with different block codes",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Rows and columns of a unit's raster that may hold nonzero AC, from its
/// entropy decoder's per-unit count (`eobs`): the run-level EOB, or the
/// split-Huffman group count (groups of four zigzag positions).
#[inline]
fn unit_extent(t: &Tables, mode: LossyEntropy, e: u8) -> (u8, u8) {
    match mode {
        LossyEntropy::RunLevel => t.zz_ext[e as usize & 63],
        LossyEntropy::SplitHuf => t.zz_ext[(e as usize * 4).min(63)],
    }
}

/// Reconstructs the merged block of shape `s` whose top-left parent is
/// block (`r`, `x0`) of a unit `bw` blocks wide into `dst`: per plane the
/// parents' DCs become the LLF coefficients, the quadrant origins and the
/// units' rows the rest of the pre-scaled spectrum (SPEC 2.9.4), then the
/// inverse ([`kernel::group`]). `aq` is the block's AC multiplier (1 without
/// AQ). Zeroes every coefficient of the parents.
#[allow(clippy::too_many_arguments)]
pub(super) fn recon_merged(
    t: &Tables,
    mode: LossyEntropy,
    s: Shape,
    q: &mut [i32],
    eobs: &[u8],
    (r, x0, bw): (usize, usize, usize),
    aq: f32,
    dst: &mut kernel::Bands<'_>,
    simd: bool,
) {
    let (ph, pw) = (s.ph(), s.pw());
    let srcs: [kernel::Src; NCH] = core::array::from_fn(|ch| {
        let dq = &t.dq[ch][s as usize];
        let sc = &t.sc[s as usize];
        let mut src = kernel::Src::default();
        let mut dcs = [[0.0f32; 2]; 2];
        let (mut cn, mut vr) = (pw, [ph, 0]);
        let mut ac = false;
        for py in 0..ph {
            for px in 0..pw {
                let p = py * pw + px;
                let b = (r + py) * bw + x0 + px;
                let base = (b * NCH + ch) * 64;
                src.base[p] = base;
                dcs[py][px] = q[base] as f32 * t.dc_step[ch];
                let (er, ec) = unit_extent(t, mode, eobs[b * NCH + ch]);
                src.er[p] = er;
                if er > 0 {
                    ac = true;
                    vr[px] = vr[px].max(py * 8 + er as usize);
                    cn = cn.max(px * 8 + ec as usize);
                }
                if p > 0 {
                    let o = q[(r * bw + x0) * NCH * 64 + ch * 64 + py * 8 + px];
                    if o != 0 {
                        src.ori[p] = o as f32 * (dq[py * 8 * 16 + px * 8] * aq);
                        vr[px] = vr[px].max(py * 8 + 1);
                        cn = cn.max(px * 8 + 1);
                    }
                }
            }
        }
        let l = dcs_to_llf(s, dcs);
        let mut llf_ac = false;
        for v in 0..ph {
            for u in 0..pw {
                src.llf[v][u] = l[v][u] * sc[v * 16 + u];
                llf_ac |= (v, u) != (0, 0) && src.llf[v][u] != 0.0;
            }
        }
        src.vr = [vr[0] as u8, vr[1] as u8];
        src.cn = cn as u8;
        src.flat = !ac && !llf_ac;
        src
    });
    let dqs: [&[f32; 256]; NCH] = core::array::from_fn(|ch| &t.dq[ch][s as usize]);
    kernel::group(s, q, &srcs, &dqs, aq, dst, simd);
}

/// Column panel width of the merged traversal: the reconstruction chunk
/// width, even and at least 2 (a 16-wide merge starts at an even column, so
/// it never spans two panels).
fn panel_blocks<const BOUNDED: bool>(bw: usize) -> usize {
    let cw = if BOUNDED {
        RECON_CHUNK
    } else {
        recon_chunk_blocks()
    };
    if bw < 2 { 1 } else { cw.min(bw).max(2) & !1 }
}

/// Reconstructs stripe `s` of a unit with merged blocks (`roles`: one role
/// per parent) into `out`: column panels of [`panel_blocks`], all block
/// rows of a panel before the next. Within a panel two bands ping-pong per
/// block row: a 16-tall merge writes its lower half into the band that
/// becomes the next block row's, so no row is copied. Coded runs, kept and
/// exact blocks, the flat fast path and the color stage are those of
/// [`recon_stripe_mode`].
#[allow(clippy::too_many_arguments)]
pub(super) fn recon_stripe<const BOUNDED: bool>(
    p: &Reconstruction<'_>,
    t: &Tables,
    g: &Geom,
    s: usize,
    aq: &[f32],
    roles: &[u8],
    out: &mut [&mut [u8]],
    simd: bool,
    q: &mut [i32],
    eobs: &[u8],
    planes: &mut Vec<f32>,
    lin: &mut Vec<f32>,
    small: simd::Small<'_>,
) {
    let (by0, by1) = g.rows(s);
    let pb = panel_blocks::<BOUNDED>(g.bw);
    let pw = pb * 8;
    // Padded by 64 bytes against 4K aliasing (see `recon_stripe_mode`).
    let ps = pw + 16;
    // Both bands in one buffer (no second scratch vector).
    let band = NCH * 8 * ps;
    planes.resize(2 * band, 0.0);
    let float_out =
        matches!(p.info.sample, SampleType::F16 | SampleType::F32) || p.xfer16.is_some();
    lin.resize(if float_out { pw * 3 } else { 0 }, 0.0);
    let kind = InputKind::of(p.info).unwrap();
    let wide = kind == InputKind::U16;
    let xyb16 = p.xfer16.as_ref();
    let bps = p.info.sample.bytes();
    let skip = p.code0_skips;
    let clear_q = p.clear_q;
    let mode = p.mode;
    let fused_simd = {
        #[cfg(feature = "lab")]
        {
            simd && !small.int && !super::IDCT_FMA.load(core::sync::atomic::Ordering::Relaxed)
        }
        #[cfg(not(feature = "lab"))]
        {
            simd && !small.int
        }
    };
    let (a, b) = planes.split_at_mut(band);
    let (mut cur, mut nxt): (&mut [f32], &mut [f32]) = (a, b);
    for px0 in (0..g.bw).step_by(pb) {
        let px1 = (px0 + pb).min(g.bw);
        for r in 0..by1 - by0 {
            let aqr = if aq.is_empty() {
                &[][..]
            } else {
                &aq[r * g.bw..(r + 1) * g.bw]
            };
            let y0 = (by0 + r) * 8;
            let rows = 8.min(g.h - y0);
            let rl = &roles[r * g.bw..(r + 1) * g.bw];
            // Runs of coded blocks inside the panel; kept and exact blocks
            // (multiplier 0) are skipped and their coefficients cleared.
            let mut bx = px0;
            while bx < px1 {
                if skip && aqr[bx] == 0.0 {
                    if clear_q {
                        let b = r * g.bw + bx;
                        q[b * NCH * 64..(b + 1) * NCH * 64].fill(0);
                    }
                    bx += 1;
                    continue;
                }
                let a = bx;
                while bx < px1 && !(skip && aqr[bx] == 0.0) {
                    bx += 1;
                }
                let (bx0, bx1) = (a, bx);
                let tm = prof::start();
                let tm = if rl[bx0..bx1].iter().any(|&m| m != 0) {
                    tm
                } else {
                    let Err(tm) = flat_run(
                        p,
                        g,
                        by0,
                        r,
                        (bx0, bx1),
                        q,
                        eobs,
                        out,
                        lin,
                        (kind, wide, xyb16, simd),
                        tm,
                    ) else {
                        continue;
                    };
                    tm
                };
                if fused_simd {
                    kernel::recon_run(
                        p.dq,
                        p.bias.as_deref(),
                        t,
                        mode,
                        q,
                        eobs,
                        r,
                        g.bw,
                        (bx0, bx1),
                        px0,
                        aqr,
                        rl,
                        cur,
                        nxt,
                        ps,
                        small,
                        clear_q,
                    );
                } else {
                    // Unmerged spans through the 8x8 path, merged blocks at their
                    // top row through the merged kernel.
                    let mut x = bx0;
                    while x < bx1 {
                        let Some(sh) = Shape::from_role(rl[x]) else {
                            let e = x + rl[x..bx1].iter().take_while(|&&m| m == 0).count();
                            recon8(
                                p,
                                q,
                                eobs,
                                r,
                                g.bw,
                                (x, e),
                                aqr,
                                cur,
                                (x - px0) * 8,
                                ps,
                                simd,
                                small,
                            );
                            x = e;
                            continue;
                        };
                        debug_assert_eq!((x - px0) % sh.pw(), 0);
                        if r % sh.ph() == 0 {
                            let a = aqr.get(x).copied().unwrap_or(1.0);
                            let mut dst = kernel::Bands {
                                top: cur,
                                bot: nxt,
                                stride: ps,
                                off: (x - px0) * 8,
                            };
                            recon_merged(t, mode, sh, q, eobs, (r, x, g.bw), a, &mut dst, simd);
                        }
                        x += sh.pw();
                    }
                }
                tm.add(2);
                let tm = prof::start();
                let x0 = bx0 * 8;
                let n = (bx1 * 8).min(g.w) - x0;
                let rb = n * 3 * bps;
                let xb = (bx0 - px0) * 8;
                for y in 0..rows {
                    let row = &mut out[y0 - by0 * 8 + y][x0 * 3 * bps..][..rb];
                    let pl = |c: usize| &cur[(c * 8 + y) * ps + xb..];
                    color_chunk_row(p, kind, wide, xyb16, simd, pl(0), pl(1), pl(2), lin, row, n);
                }
                tm.add(3);
            }
            core::mem::swap(&mut cur, &mut nxt);
        }
    }
}

/// The 8x8 (and 4x4, `small`) reconstruction of blocks `a..b` of block row
/// `r` into `planes` at column `x` (block `a`'s first column), plane row
/// stride `ps`.
#[allow(clippy::too_many_arguments)]
pub(super) fn recon8(
    p: &Reconstruction<'_>,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    (a, b): (usize, usize),
    aqr: &[f32],
    planes: &mut [f32],
    x: usize,
    ps: usize,
    simd: bool,
    small: simd::Small<'_>,
) {
    // The reconstruction bias (Lossy 0.4) applies to these 8x8 blocks, as
    // on the plain 8x8 path; merged blocks and 4x4 planes reconstruct
    // without it (SPEC 2.9, 2.11).
    let bias = p.bias.as_deref();
    if simd && !small.int {
        // The SIMD row kernel places block bx at (bx - a) * 8.
        let pl = &mut planes[x..];
        return simd::recon_block_row(
            q, eobs, r, bw, a, b, p.dq, aqr, bias, pl, ps, small, p.clear_q,
        );
    }
    for bx in a..b {
        let blk_i = r * bw + bx;
        let split = small.split.get(blk_i).copied().unwrap_or(0);
        for ch in 0..NCH {
            let mut blk = [0.0f32; 64];
            let qq = &mut q[(blk_i * NCH + ch) * 64..][..64];
            if split >> ch & 1 != 0 {
                let dqs = aq_dequant(&small.dq[ch], aqr.get(bx).copied());
                for i in 0..64 {
                    blk[i] = qq[i] as f32 * dqs[i];
                }
                #[cfg(feature = "lab")]
                if small.int {
                    blk = super::small_lab::inverse_int_ref(&blk);
                } else {
                    small::inverse_ref(&mut blk);
                }
                #[cfg(not(feature = "lab"))]
                small::inverse_ref(&mut blk);
                if p.clear_q {
                    qq.fill(0);
                }
                for y in 0..8 {
                    planes[(ch * 8 + y) * ps + x + (bx - a) * 8..][..8]
                        .copy_from_slice(&blk[y * 8..y * 8 + 8]);
                }
                continue;
            }
            let dqs = aq_dequant(&p.dq[ch], aqr.get(bx).copied());
            match bias {
                // Lossy 0.4: (q - sign(q) * B) * step, as `recon_stripe_mode`.
                Some(bt) => {
                    for i in 0..64 {
                        let s = qq[i].clamp(-1, 1);
                        let b = bt[ch][(qq[i] != s) as usize][i];
                        blk[i] = (qq[i] as f32 - s as f32 * b) * dqs[i];
                    }
                }
                None => {
                    for i in 0..64 {
                        blk[i] = qq[i] as f32 * dqs[i];
                    }
                }
            }
            dct::idct8x8(&mut blk);
            if p.clear_q {
                qq.fill(0);
            }
            for y in 0..8 {
                planes[(ch * 8 + y) * ps + x + (bx - a) * 8..][..8]
                    .copy_from_slice(&blk[y * 8..y * 8 + 8]);
            }
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    /// cos(m pi / 32) in f64.
    fn cos32(m: usize) -> f64 {
        (m as f64 * core::f64::consts::PI / 32.0).cos()
    }

    #[test]
    fn llf_r_is_the_rounded_exact_ratio() {
        let exact = 1.0 / (8.0 * core::f64::consts::SQRT_2 * (core::f64::consts::PI / 32.0).sin());
        assert_eq!(
            LLF_R,
            exact as f32,
            "{exact} vs {LLF_R} {:#x}",
            (exact as f32).to_bits()
        );
        // The same ratio from the orthonormal bases' sums: the 16-point
        // coefficient 1 of a signal 1 on the first half and 0 on the second,
        // over the 2-point coefficient 1 of the halves' 8-point DCs.
        let s1: f64 = (0..8)
            .map(|n| core::f64::consts::FRAC_1_SQRT_2 * 0.5 * cos32(2 * n + 1))
            .sum();
        let s0 = 8.0 / 8f64.sqrt();
        assert!((s1 / (s0 * core::f64::consts::FRAC_1_SQRT_2) - exact).abs() < 1e-12);
    }

    #[test]
    fn quadrant_slots_cover_every_ac_coefficient_once() {
        for s in Shape::ALL {
            let mut seen = [[false; 64]; 4];
            for v in 0..s.h() {
                for u in 0..s.w() {
                    if v < s.ph() && u < s.pw() {
                        continue;
                    }
                    let (p, z) = slot(s, v, u);
                    assert!(p < s.ph() * s.pw());
                    assert!(!seen[p][z], "{s:?} {v} {u}");
                    seen[p][z] = true;
                }
            }
            // Every slot of every unit except the parents' DCs.
            for p in 0..s.ph() * s.pw() {
                for z in 0..64 {
                    assert_eq!(seen[p][z], z != 0, "{s:?} unit {p} slot {z}");
                }
            }
        }
    }

    #[test]
    fn steps_take_the_physical_frequency() {
        let t: [f32; 64] = core::array::from_fn(|i| 1.0 + i as f32);
        // On table entries: the entry.
        assert_eq!(step(&t, Shape::S16x16, 4, 6), t[2 * 8 + 3]);
        assert_eq!(step(&t, Shape::S16x8, 5, 6), t[5 * 8 + 3]);
        assert_eq!(step(&t, Shape::S8x16, 6, 5), t[3 * 8 + 5]);
        // Halfway: the mean; past the last entry: the last entry.
        assert_eq!(step(&t, Shape::S16x16, 0, 3), (t[1] + t[2]) / 2.0);
        assert_eq!(step(&t, Shape::S16x16, 15, 15), t[63]);
        assert_eq!(
            step(&t, Shape::S16x16, 3, 3),
            ((t[9] + t[10]) / 2.0 + (t[17] + t[18]) / 2.0) / 2.0
        );
    }

    #[test]
    fn llf_maps_piecewise_constant_blocks_to_parent_means() {
        // A merged block whose parents are constant: its LLF coefficients
        // through `llf_to_dcs` give each parent's own 8x8 DC (8 x mean).
        let m16: [[f64; 16]; 16] = core::array::from_fn(|k| {
            core::array::from_fn(|n| {
                let ck = if k == 0 {
                    0.25
                } else {
                    core::f64::consts::FRAC_1_SQRT_2 * 0.5
                };
                ck * ((k * (2 * n + 1)) as f64 * core::f64::consts::PI / 32.0).cos()
            })
        });
        let m8: [[f64; 8]; 8] = core::array::from_fn(|k| {
            core::array::from_fn(|n| {
                let ck = if k == 0 { 8f64.sqrt().recip() } else { 0.5 };
                ck * ((k * (2 * n + 1)) as f64 * core::f64::consts::PI / 16.0).cos()
            })
        });
        let means = [[3.0f64, -7.5], [12.25, 0.5]];
        for s in Shape::ALL {
            let basis = |n: usize, k: usize, i: usize| if n == 16 { m16[k][i] } else { m8[k][i] };
            let mut llf = [[0.0f32; 2]; 2];
            for v in 0..s.ph() {
                for u in 0..s.pw() {
                    let mut acc = 0.0f64;
                    for y in 0..s.h() {
                        for x in 0..s.w() {
                            acc += means[y / 8][x / 8] * basis(s.h(), v, y) * basis(s.w(), u, x);
                        }
                    }
                    llf[v][u] = acc as f32;
                }
            }
            let d = llf_to_dcs(s, llf);
            for py in 0..s.ph() {
                for px in 0..s.pw() {
                    let want = (means[py][px] * 8.0) as f32;
                    assert!(
                        (d[py][px] - want).abs() < 1e-4,
                        "{s:?} {py} {px}: {} vs {want}",
                        d[py][px]
                    );
                }
            }
            let back = dcs_to_llf(s, d);
            for v in 0..s.ph() {
                for u in 0..s.pw() {
                    assert!((back[v][u] - llf[v][u]).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn partitions_and_blocks_agree() {
        for (sym, roles) in PARTITIONS.iter().enumerate() {
            let mut from_blocks = [0u8; 4];
            for &(s, py, px) in BLOCKS[sym] {
                for dy in 0..s.ph() {
                    for dx in 0..s.pw() {
                        let i = (py + dy) * 2 + px + dx;
                        assert_eq!(from_blocks[i], 0);
                        from_blocks[i] = s.role();
                    }
                }
            }
            assert_eq!(&from_blocks, roles, "symbol {sym}");
            assert_eq!(symbol_of(*roles), Some(sym as u8));
        }
    }
}
