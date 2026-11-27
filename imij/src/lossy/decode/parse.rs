//! Lossy stream header/table parsing, region geometry, and ROI decoding.

use super::recon::*;
use crate::lossy::*;

// ---- decoding ---------------------------------------------------------------

pub(crate) struct LossyParsed<'a> {
    pub info: ImageInfo,
    /// Bytes of the `DATA` payload.
    pub(in crate::lossy) payload_len: usize,
    pub(in crate::lossy) g: Geom,
    pub(in crate::lossy) color: LossyColor,
    pub(in crate::lossy) inv_scale: [f32; 3],
    pub(in crate::lossy) native_quantizer: bool,
    pub(in crate::lossy) dq: [[f32; 64]; 3],
    /// Lossy 0.4 reconstruction bias per channel, raster order: B for
    /// |q| = 1 and for |q| >= 2 ([`ReconBias::tables`]); `None` without one.
    pub(in crate::lossy) bias: Option<alloc::boxed::Box<[[[f32; 64]; 2]; 3]>>,
    /// Lossy 0.4 signalled linear color transform (its decode matrix,
    /// `rgb_j = M_j0*c0 + M_j1*c1 + M_j2*c2 + O_j`); `None` = the color's
    /// default (XYB, or the YCoCg lifting, including the YCoCg preset id).
    pub(in crate::lossy) transform: Option<alloc::boxed::Box<[[f32; 4]; 3]>>,
    pub(in crate::lossy) layouts: [LossyChannelLayout; 3],
    pub(in crate::lossy) layout_steps: [[[f32; 64]; 2]; 3],
    pub(in crate::lossy) adaptive_mask: u8,
    pub(in crate::lossy) compact_states: bool,
    pub(in crate::lossy) local_ans_contexts: bool,
    pub(in crate::lossy) coding_defaults: Option<coding_defaults::Defaults>,
    pub(in crate::lossy) model_refs: Option<model_refs::Models>,
    pub(in crate::lossy) uniform_dc_k: usize,
    pub(crate) dc_coverage: Option<alloc::sync::Arc<dc_coverage::Coverage>>,
    #[cfg(all(feature = "lab", test))]
    pub(in crate::lossy) catalog_lab: Option<model_candidates::Catalog>,
    pub(in crate::lossy) tables: Tables,
    /// SplitHuf: Huffman tables, their decode tables, width-table rows.
    pub(in crate::lossy) huf: Option<(huf::HufTables, huf::Dec, huf::WidRows)>,
    /// rANS decode tables (absent in modes without rANS lanes).
    pub(in crate::lossy) rans: Option<RansTables>,
    /// Nested lossless DC image (its stripes from the records); taken by a
    /// streaming decoder.
    pub(in crate::lossy) dc: Option<crate::v1::Parsed<'a>>,
    /// Per-block AQ step multipliers present (4-channel DC image).
    pub(in crate::lossy) aq: bool,
    /// 16-bit XYB output: transfer table and linear clamp.
    pub(in crate::lossy) xfer16: Option<(Vec<f32>, f32)>,
    /// Nested lossless alpha image (empty without alpha).
    pub(in crate::lossy) alpha: Option<crate::v1::Parsed<'a>>,
    /// Column group width in pixels (0 = none).
    pub(in crate::lossy) gcols: usize,
    /// Block code 0 keeps the reference block (inter frames only).
    pub(in crate::lossy) skip: bool,
    /// The unit table hint (head record 10, or a filled reservation, SPEC
    /// 2.7): the bytes of each main record's span, when the head carries one.
    pub(crate) hint: Option<Vec<u64>>,
    /// The unit table reservation (head record 12), if any.
    pub(crate) reserve: Option<crate::parse::records::Reserve>,
    /// Whether main unit records carry 0-based ordinals (head record 37, SPEC 2.7).
    pub(crate) unit_ordinals: bool,
    /// Records of the units (stripe, column group), stripe-major.
    pub(in crate::lossy) stripes: Vec<&'a [u8]>,
    /// Exact blocks (block code 1): the nested block-list image and each
    /// unit's list range.
    pub(in crate::lossy) exact: Option<exact::Exact<'a>>,
    /// The merged blocks' dequantization tables (superblocks: head record
    /// 21, or record 23 in a file that declares `superblocks`).
    pub(in crate::lossy) sb: Option<alloc::boxed::Box<superblock::Tables>>,
    /// Which partition map starts every unit record.
    pub(in crate::lossy) map: MapKind,
    /// Small transforms (head record 23): the 4x4 planes' dequantization
    /// factors ([`small::dequant4`]).
    pub(in crate::lossy) small: Option<alloc::boxed::Box<[[f32; 64]; NCH]>>,
    /// LAB: the 4x4 planes use the integer candidate (`small_lab.rs`).
    pub(in crate::lossy) small_int: bool,
}

/// The partition map at the start of every lossy unit record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapKind {
    /// No map (neither record 21 nor 23).
    None,
    /// Superblock merges (record 21, SPEC 2.9.2).
    Merges,
    /// Merges and 4x4 split flags (record 23, SPEC 2.11.2).
    Joint,
}

/// Decode tables of the rANS lanes.
pub(in crate::lossy) struct RansTables {
    pub(in crate::lossy) dec: alloc::boxed::Box<[u32; coef::MAX_TABLES << LOG_M]>,
    #[cfg_attr(
        not(any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "wasm32"
        )),
        allow(dead_code)
    )]
    pub(in crate::lossy) cmap: alloc::boxed::Box<[u8; coef::NRAW_PAD]>,
    /// `cmap` widened to u32 (hardware-gather table).
    #[cfg(target_arch = "x86_64")]
    pub(in crate::lossy) cmap32: alloc::boxed::Box<[u32; coef::NRAW_PAD]>,
}

impl RansTables {
    pub(in crate::lossy) fn new(tables: &Tables) -> Self {
        let cmap = tables.padded_map();
        Self {
            dec: tables.dec_tables(),
            #[cfg(target_arch = "x86_64")]
            cmap32: cmap
                .iter()
                .map(|&v| u32::from(v))
                .collect::<Vec<_>>()
                .into_boxed_slice()
                .try_into()
                .unwrap(),
            cmap,
        }
    }
}

/// Planning allowances live beside the structures/constants they account for.
pub(crate) fn decoder_table_bytes() -> u128 {
    // Fixed 64-bit-sized allowance keeps encoder plans identical on wasm32.
    const HEADER_ALLOWANCE: usize = 16 * 1024;
    const _: () = assert!(core::mem::size_of::<LossyParsed<'static>>() <= HEADER_ALLOWANCE);
    let rans = core::mem::size_of::<[u32; coef::MAX_TABLES << LOG_M]>() + coef::NRAW_PAD * 5;
    let huffman = (huf::MAX_CNT_TABLES + huf::MAX_WID_TABLES) * (1 << huf::LOG) * 2;
    let models = coef::MAX_TABLES * (2 * coef::ALPHA * 4 + 64) + 2 * coef::NRAW_PAD;
    (rans.max(huffman) + models + 2 * HEADER_ALLOWANCE + 32 * 1024) as u128
}

pub(crate) fn decoder_recon_bytes() -> u128 {
    // Vec growth, plane padding, linear color row and run descriptors. The
    // public decoder uses RECON_CHUNK; lab-only overrides are outside the plan.
    (2 * ((RECON_CHUNK * 8 + 16) * 8 * NCH * 4 + RECON_CHUNK * 8 * 3 * 4)) as u128
}

impl LossyParsed<'_> {
    #[cfg(feature = "v3")]
    pub(crate) fn has_model_references(&self) -> bool {
        self.model_refs.is_some()
    }

    #[cfg(feature = "v3")]
    pub(crate) fn model_storage_bytes(&self) -> u128 {
        self.model_refs
            .as_ref()
            .map_or(0, model_refs::Models::storage_bytes)
    }

    #[cfg(any(feature = "v3", test))]
    pub(crate) fn model_prefix<'b>(
        &self,
        u: usize,
        role: u64,
        bytes: &'b [u8],
    ) -> Result<&'b [u8], Error> {
        if role == 0 {
            return Ok(bytes);
        }
        let (_, g) = self.group(u % self.ngroups());
        Ok(self.record(&g, u / self.ngroups(), bytes)?.1)
    }

    /// 0: no leading map; 1: length-delimited map; 2: legacy packed layouts.
    #[cfg(feature = "v3")]
    pub(crate) fn model_map_prefix(&self, u: usize) -> (u8, usize) {
        if self.coding_defaults.is_some() || self.map != MapKind::None {
            return (1, 0);
        }
        let (_, g) = self.group(u % self.ngroups());
        let (a, b) = g.rows(u / self.ngroups());
        (if self.adaptive_mask == 0 { 0 } else { 2 }, (b - a) * g.bw)
    }

    #[cfg(feature = "v3")]
    pub(crate) fn referenced_model(
        &self,
        ordinal: u64,
        role: u64,
        bytes: &[u8],
    ) -> Result<Option<u64>, Error> {
        self.model_refs
            .as_ref()
            .map_or(Ok(None), |m| m.dependency(ordinal, role, bytes))
    }

    #[cfg(feature = "v3")]
    pub(crate) fn admit_model_prefix(
        &mut self,
        ordinal: u64,
        role: u64,
        bytes: &[u8],
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<(), Error> {
        if let Some(models) = &mut self.model_refs {
            models.admit(
                ordinal,
                role,
                bytes,
                self.dc.as_ref().ok_or(Error::Corrupt("missing DC head"))?,
                admission,
            )?;
        }
        Ok(())
    }

    #[cfg(feature = "v3")]
    pub(crate) fn check_first_use_prefix(&self, role: u64, bytes: &[u8]) -> Result<(), Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, _) = model_refs::prefix(&mut r, role, self.tables.mode)?;
        if !matches!(selector, model_refs::Selector::Define(_)) || r.remaining() != 0 {
            return Err(Error::Corrupt(
                "reference target is not a first-use definition",
            ));
        }
        Ok(())
    }

    #[cfg(feature = "v3")]
    pub(crate) fn model_uses_ans(&self) -> bool {
        self.tables.mode == LossyEntropy::RunLevel
    }

    #[cfg(all(feature = "lab", feature = "std"))]
    pub(crate) fn component_requires_reference(&self) -> bool {
        self.skip
    }

    /// Move the owned head after the caller has retained its record spans.
    /// Borrowed unit records are intentionally discarded.
    pub(crate) fn without_records(self) -> LossyParsed<'static> {
        LossyParsed {
            info: self.info,
            payload_len: self.payload_len,
            g: self.g,
            color: self.color,
            inv_scale: self.inv_scale,
            native_quantizer: self.native_quantizer,
            dq: self.dq,
            bias: self.bias,
            transform: self.transform,
            layouts: self.layouts,
            layout_steps: self.layout_steps,
            adaptive_mask: self.adaptive_mask,
            compact_states: self.compact_states,
            local_ans_contexts: self.local_ans_contexts,
            coding_defaults: self.coding_defaults,
            model_refs: self.model_refs,
            dc_coverage: self.dc_coverage,
            uniform_dc_k: self.uniform_dc_k,
            #[cfg(all(feature = "lab", test))]
            catalog_lab: self.catalog_lab,
            tables: self.tables,
            huf: self.huf,
            rans: self.rans,
            dc: self.dc.map(crate::v1::Parsed::without_records),
            aq: self.aq,
            xfer16: self.xfer16,
            alpha: self.alpha.map(crate::v1::Parsed::without_records),
            gcols: self.gcols,
            skip: self.skip,
            hint: self.hint,
            reserve: self.reserve,
            unit_ordinals: self.unit_ordinals,
            stripes: Vec::new(),
            exact: self.exact.map(exact::Exact::without_records),
            sb: self.sb,
            map: self.map,
            small: self.small,
            small_int: self.small_int,
        }
    }

    pub(crate) fn needs_reference(&self) -> bool {
        self.skip
    }

    /// A unit record's selector map and the rest of its bytes. With
    /// superblocks or small transforms the record starts with its partition
    /// map instead, which this skips (layouts are the defaults then:
    /// [`recon_unit`] reads it where the unit is reconstructed).
    pub(in crate::lossy) fn record<'a>(
        &self,
        g: &Geom,
        s: usize,
        bytes: &'a [u8],
    ) -> Result<(layout::ModeMap<'a>, &'a [u8]), Error> {
        if self.coding_defaults.is_some() {
            return Ok((
                layout::ModeMap::Constant(0),
                coding_defaults::Defaults::split(bytes)?.1,
            ));
        }
        if self.map != MapKind::None {
            return Ok((layout::ModeMap::Constant(0), superblock::skip_map(bytes)?));
        }
        let (a, b) = g.rows(s);
        layout::read_modes(bytes, (b - a) * g.bw, self.adaptive_mask)
    }
    // The rANS modes always carry their tables (`parse_lossy`).
    #[cfg(any(
        all(target_arch = "x86_64", feature = "avx512"),
        all(feature = "lab", feature = "std")
    ))]
    pub(in crate::lossy) fn dec(&self) -> &[u32; coef::MAX_TABLES << LOG_M] {
        &self.rans.as_ref().expect("rANS tables").dec
    }
    #[cfg(any(
        all(target_arch = "x86_64", feature = "avx512"),
        all(feature = "lab", feature = "std")
    ))]
    pub(in crate::lossy) fn cmap(&self) -> &[u8; coef::NRAW_PAD] {
        &self.rans.as_ref().expect("rANS tables").cmap
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    pub(in crate::lossy) fn cmap32(&self) -> &[u32; coef::NRAW_PAD] {
        &self.rans.as_ref().expect("rANS tables").cmap32
    }
}

/// Tests and lab tools: the payload of a lossy file, parsed under its
/// header's color and every feature this decoder knows.
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn parse_lossy(file: &[u8]) -> Result<LossyParsed<'_>, Error> {
    let (info, color, body, _) =
        crate::v1::lossy_body(file)?.ok_or(Error::InvalidInput("not a lossy file"))?;
    let version = crate::parse::header::read_header(&mut ByteReader::new(file))?.version;
    parse_lossy_as(
        info,
        version,
        color,
        crate::parse::header::Features::ALL,
        body,
    )
}

/// Parses a lossy `DATA` payload of a file whose header has internal color
/// `color` and declares the features `lossy` (seed tables need
/// `seed_bank_1`, exact blocks `exact_blocks`).
pub(crate) fn parse_lossy_as<'a>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &'a [u8],
) -> Result<LossyParsed<'a>, Error> {
    parse_lossy_checked(
        info,
        version,
        color,
        lossy,
        body,
        &crate::parse::limits::Unrestricted,
    )
}

pub(crate) fn parse_lossy_checked<'a>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    let parsed = parse_lossy_prefix_checked(info, version, color, lossy, body, admission)?;
    if parsed.payload_len != body.len() {
        return Err(Error::Corrupt("trailing DATA bytes"));
    }
    Ok(parsed)
}

pub(in crate::lossy) fn parse_lossy_prefix_checked<'a>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &'a [u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    if version == 4 {
        return Err(Error::Unsupported(
            "V4 requires its declared payload driver",
        ));
    }
    let mut r = ByteReader::new(body);
    let mut p = parse_lossy_head_checked(info, color, lossy, &mut r, admission)?;
    // Records in decode order (see `encode_lossy`, SPEC 2.7), checked
    // against the hint.
    use crate::parse::records::{R_AC, R_ALPHA, R_DC, R_EXACT};
    let ns = p.g.stripes();
    let ng = p.ngroups();
    let map = p.unit_map();
    let units = ns.checked_mul(ng).ok_or(Error::TooLarge)?;
    if units > r.remaining() / 2 {
        return Err(Error::Corrupt("lossy unit count exceeds remaining records"));
    }
    p.stripes
        .try_reserve_exact(units)
        .map_err(|_| Error::TooLarge)?;
    let mut records = crate::parse::records::Split::with_ordinals(
        &mut r,
        version,
        p.hint.take(),
        units,
        p.unit_ordinals,
    )?;
    let mut ordinal = 0u64;
    for u in 0..ns * ng {
        admission.check()?;
        let si = u / ng;
        if map.starts_dc_at(si, u % ng) {
            let bytes = records.next(R_DC)?;
            if let Some(models) = &mut p.model_refs {
                models.admit(ordinal, 0, bytes, p.dc.as_ref().unwrap(), admission)?;
            }
            p.dc.as_mut().unwrap().stripes.push(bytes);
            ordinal += 1;
        }
        if let Some(a) = &mut p.alpha {
            a.stripes.push(records.next(R_ALPHA)?);
            ordinal += 1;
        }
        let bytes = records.next(R_AC)?;
        let (_, gg) = p.group(u % ng);
        let model_bytes = p.record(&gg, si, bytes)?.1;
        if let Some(models) = &mut p.model_refs {
            models.admit(ordinal, 1, model_bytes, p.dc.as_ref().unwrap(), admission)?;
        }
        p.stripes.push(bytes);
        ordinal += 1;
        if let Some(e) = &mut p.exact {
            e.push_record(records.next(R_EXACT)?)?;
            ordinal += 1;
        }
    }
    records.finish()?;
    if let Some(e) = &p.exact {
        e.finish()?;
    }
    p.payload_len = r.pos;
    Ok(p)
}

impl LossyParsed<'_> {
    /// Lossy stripes (units per column group).
    pub(crate) fn stripe_count(&self) -> usize {
        self.g.stripes()
    }

    /// Where each unit's records sit (SPEC 2.7).
    pub(crate) fn unit_map(&self) -> crate::parse::records::UnitMap {
        crate::parse::records::UnitMap::lossy(
            self.ngroups(),
            self.dc_k(),
            self.alpha.is_some(),
            self.exact.is_some(),
        )
        .with_coverage(self.dc_coverage.clone())
    }

    /// The payload codes some blocks exactly (lossy head record 5).
    #[cfg(feature = "v3")]
    pub(crate) fn has_exact_blocks(&self) -> bool {
        self.exact.is_some()
    }

    /// The exact blocks' head: every unit's list entries and every nested
    /// stripe's place (region decoding).
    #[cfg(feature = "v3")]
    pub(crate) fn exact(&self) -> Option<&exact::Exact<'_>> {
        self.exact.as_ref()
    }

    /// Lossy stripes per DC stripe.
    pub(crate) fn dc_k(&self) -> usize {
        self.uniform_dc_k
    }

    pub(crate) fn dc_span(&self, s: usize, g: usize) -> dc_coverage::Span {
        self.dc_coverage.as_ref().map_or_else(
            || {
                let k = self.dc_k();
                dc_coverage::Span {
                    start: s / k * k,
                    end: ((s / k + 1) * k).min(self.g.stripes()),
                    unit: s / k * self.ngroups() + g,
                }
            },
            |c| c.span(s, g),
        )
    }

    /// Column groups per stripe.
    pub(crate) fn ngroups(&self) -> usize {
        if self.gcols == 0 {
            1
        } else {
            self.g.w.div_ceil(self.gcols)
        }
    }

    /// First pixel column and geometry of column group `gi` (a group
    /// decodes like an image as wide as the group).
    pub(in crate::lossy) fn group(&self, gi: usize) -> (usize, Geom) {
        if self.gcols == 0 {
            return (
                0,
                Geom::new(self.g.w as u32, self.g.h as u32, self.g.sb as u32),
            );
        }
        let x0 = gi * self.gcols;
        let gw = self.gcols.min(self.g.w - x0);
        (x0, Geom::new(gw as u32, self.g.h as u32, self.g.sb as u32))
    }
}

/// Unit geometry of a lossy payload, for region decoding (`crate::region`).
#[cfg(any(feature = "v3", feature = "lab"))]
pub(crate) struct RegionGeometry {
    /// Pixel rows per lossy stripe.
    pub rows: usize,
    pub stripes: usize,
    pub groups: usize,
    /// Column group width in pixels (0 = none).
    pub gcols: usize,
    /// Lossy stripes per DC stripe.
    pub k: usize,
    pub alpha: bool,
}

impl<'a> LossyParsed<'a> {
    #[cfg(any(feature = "v3", feature = "lab"))]
    pub(crate) fn region_geometry(&self) -> RegionGeometry {
        RegionGeometry {
            rows: self.g.sb * 8,
            stripes: self.g.stripes(),
            groups: self.ngroups(),
            gcols: self.gcols,
            k: self.dc_k(),
            alpha: self.alpha.is_some(),
        }
    }
}

/// Region decoding (`crate::region`): the working memory one work item of
/// column groups `g0..g1` holds while it decodes (the widest group counts):
/// one stripe's coefficients ([`crate::decoder_memory::lossy_stripe_bytes`],
/// the estimate the decode planner charges), reconstruction scratch, its
/// staged RGB and alpha rows, its DC rows and values, the prediction
/// state of the nested DC and alpha units it decodes, and with exact blocks
/// a decoded nested stripe and its decoder's state
/// ([`exact::Exact::item_bytes`]).
#[cfg(feature = "v3")]
pub(crate) fn region_unit_bytes(p: &LossyParsed<'_>, g0: usize, g1: usize) -> Result<u64, Error> {
    let dcp = p.dc.as_ref().ok_or(Error::Corrupt("no DC image"))?;
    let (sb, rows) = (p.g.sb as u128, (p.g.sb * 8) as u128);
    let bps = p.info.sample.bytes() as u128;
    let mut most = 0u128;
    for g in g0..g1 {
        let gw = p.group(g).1.w as u128;
        let mut unit = crate::decoder_memory::lossy_stripe_bytes(gw, sb)?
            + decoder_recon_bytes()
            + gw * rows * 4 * bps
            + gw.div_ceil(8) * sb * (dcp.bpp() as u128 + 16)
            + u128::from(crate::v1::region_unit_bytes(dcp, g, g + 1)?);
        if let Some(a) = &p.alpha {
            unit += u128::from(crate::v1::region_unit_bytes(a, g, g + 1)?);
        }
        if let Some(e) = &p.exact {
            unit += e.item_bytes(gw.div_ceil(8) * sb)?;
        }
        most = most.max(unit);
    }
    if p.has_model_references() {
        most += 2 * decoder_table_bytes() + crate::decoder_memory::lossless_tables(4, 32, true);
    }
    u64::try_from(most).map_err(|_| Error::TooLarge)
}

/// Region decoding (`crate::region`): decodes lossy stripes `s0..s1` of
/// column groups `g0..g1` into `out` (rows of `stride` bytes; row 0 is
/// stripe `s0`'s first pixel row, column 0 is group `g0`'s first column).
/// `dc[(d - s0 / k) * gn + g - g0]` holds the DC record of DC stripe `d`,
/// group `g` (gn = `g1 - g0`); `units[(s - s0) * gn + g - g0]` the alpha
/// (empty without alpha) and lossy records of unit (`s`, `g`); `stripes`
/// the encoded nested exact-block stripes those units need, by index. Each work
/// item decodes the region's stripes of one DC stripe and group, decoding
/// that DC stripe's rows from its top once.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "v3")]
pub(crate) fn decode_lossy_region(
    p: &LossyParsed<'_>,
    (s0, s1, g0, g1): (usize, usize, usize, usize),
    dc: &[(usize, &[u8])],
    units: &[(&[u8], &[u8])],
    stripes: &[(usize, &[u8])],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    obs: &crate::region::RegionObs<'_>,
) -> Result<(), Error> {
    // crate::region refuses keep blocks at open.
    debug_assert!(!p.skip);
    let rows = p.g.sb * 8;
    let (y0, y1) = (s0 * rows, (s1 * rows).min(p.g.h));
    let ng = p.ngroups();
    let gn = g1 - g0;
    let pxb = p.info.row_bytes() / p.g.w;
    let box_w: usize = (g0..g1).map(|g| p.group(g).1.w).sum();
    if stride < box_w * pxb || out.len() < (y1 - y0 - 1) * stride + box_w * pxb {
        return Err(Error::InvalidInput("region buffer too small"));
    }
    let dcp = p.dc.as_ref().ok_or(Error::Corrupt("no DC image"))?;
    let alpha_img = p
        .alpha
        .as_ref()
        .map(|ap| (ap, crate::v1::OutMap::new(&ap.info, &ap.layout)));
    // Exact blocks: the nested image's output map; each work item decodes
    // the stripes its units read, once.
    let exact_om = p
        .exact
        .as_ref()
        .map(|e| crate::v1::OutMap::new(&e.img.info, &e.img.layout));
    let no_check = || Ok(());
    let rgb = p.info.channels == Channels::Rgb;
    let bps = p.info.sample.bytes();
    let mut unit_rows: Vec<Vec<&mut [u8]>> = (0..units.len()).map(|_| Vec::new()).collect();
    for (i, row) in out.chunks_mut(stride).take(y1 - y0).enumerate() {
        let s = (y0 + i) / rows;
        let mut rest = &mut row[..box_w * pxb];
        for gi in g0..g1 {
            let (a, b) = rest.split_at_mut(p.group(gi).1.w * pxb);
            unit_rows[(s - s0) * gn + gi - g0].push(a);
            rest = b;
        }
    }
    // Work items: (group, the region's stripes of one DC stripe).
    let mut items: Vec<(usize, Vec<(usize, Vec<&mut [u8]>)>)> = Vec::new();
    for gi in g0..g1 {
        for s in s0..s1 {
            let r = core::mem::take(&mut unit_rows[(s - s0) * gn + gi - g0]);
            match items.last_mut() {
                Some((g, v)) if *g == gi && p.dc_span(v[0].0, gi).unit == p.dc_span(s, gi).unit => {
                    v.push((s, r))
                }
                _ => items.push((gi, alloc::vec![(s, r)])),
            }
        }
    }
    let decode_item = |_, (gi, group): (usize, Vec<(usize, Vec<&mut [u8]>)>)| {
        let mut sc = Scratch::default();
        let mut exact_st = exact::ItemState::default();
        let gg = p.group(gi).1;
        let span = p.dc_span(group[0].0, gi);
        let dr = gg.bw * dcp.bpp();
        let ordinal = p.unit_map().first(span.start * ng + gi);
        let at = dc
            .binary_search_by_key(&ordinal, |&(id, _)| id)
            .map_err(|_| Error::Corrupt("missing region DC dependency"))?;
        let (model, body) = if let Some(models) = &p.model_refs {
            models.dc(dc[at].1, dcp)?
        } else {
            (None, dc[at].1)
        };
        let dcp = model.as_deref().unwrap_or(dcp);
        let dc_om = crate::v1::OutMap::new(&dcp.info, &dcp.layout);
        let mut dc_rows = crate::v1::UnitRows::new_with(dcp, span.unit, body)?;
        let mut next = 0;
        for (s, mut rws) in group {
            obs.begin(s, gi)?;
            let nb = p.g.sb.min(p.g.bh - s * p.g.sb);
            let first = (s - span.start) * p.g.sb;
            resize_fallible(&mut sc.dc_px, nb * dr, 0)?;
            // Rows above the stripe (a region starting inside a DC stripe)
            // are decoded into row 0 and overwritten.
            while next < first + nb {
                let r = next.saturating_sub(first);
                dc_rows.next_row(dcp, &dc_om, &mut sc.dc_px[r * dr..][..dr])?;
                next += 1;
            }
            let (mut dcv, mut aqv) = (core::mem::take(&mut sc.dc), core::mem::take(&mut sc.aq));
            dcv.clear();
            aqv.clear();
            dc_values(
                dcp.info.sample,
                p.aq,
                p.exact.is_some(),
                &sc.dc_px[..nb * dr],
                &mut dcv,
                &mut aqv,
            )?;
            let (alpha_bytes, ac) = units[(s - s0) * gn + gi - g0];
            let q = core::mem::take(&mut sc.q);
            let eobs = core::mem::take(&mut sc.eobs);
            let raw = decode_stripe_raw_bytes(p, &gg, s, ac, Group::ALL, true, q, eobs)?;
            finish_coefs(p, &gg, s, &dcv, raw, &mut sc.q, &mut sc.eobs)?;
            let Scratch {
                q,
                eobs,
                planes,
                lin,
                alpha,
                tmp,
                roles,
                splits,
                ..
            } = &mut sc;
            let ws = (planes, lin, roles, splits);
            let exact =
                |rows: &mut [&mut [u8]], st: &mut exact::ItemState| match (&p.exact, &exact_om) {
                    (Some(e), Some(om)) => {
                        let nested = exact::Nested::Bytes { om, stripes };
                        let range = e.range(s * ng + gi)?;
                        exact::apply_unit(e, nested, st, range, s, &gg, &aqv, rows, &no_check)
                    }
                    _ => Ok(()),
                };
            if rgb {
                recon_unit::<false>(p, &gg, s, ac, &aqv, &mut rws, true, q, eobs, ws)?;
                exact(rws.as_mut_slice(), &mut exact_st)?;
            } else {
                let n = rws.len();
                let crb = gg.w * 3 * bps;
                let arb = gg.w * bps;
                resize_fallible(tmp, n * crb, 0)?;
                let mut trows: Vec<&mut [u8]> = tmp.chunks_mut(crb).collect();
                recon_unit::<false>(p, &gg, s, ac, &aqv, &mut trows, true, q, eobs, ws)?;
                exact(trows.as_mut_slice(), &mut exact_st)?;
                let a = match &alpha_img {
                    Some((ap, om)) => {
                        resize_fallible(alpha, n * arb, 0)?;
                        let mut st = crate::v1::UnitRows::new_with(ap, s * ng + gi, alpha_bytes)?;
                        for r in 0..n {
                            st.next_row(ap, om, &mut alpha[r * arb..][..arb])?;
                        }
                        Some(&alpha[..n * arb])
                    }
                    None => None,
                };
                pack_rows(&p.info, gg.w, tmp, a, &mut rws);
            }
            (sc.dc, sc.aq) = (dcv, aqv);
            obs.end(s, gi, &Ok(()));
        }
        Ok::<(), Error>(())
    };
    for r in crate::par::par_map(items, threads, decode_item) {
        r?;
    }
    Ok(())
}

/// The head of a lossy payload (everything before the records): no
/// stripes yet, nested images with their heads only. `color` is the
/// header's internal color (SPEC 2.8: the payload does not repeat it);
/// `admission` charges the parsed geometry and models (`Unrestricted`
/// compiles away); `declared` is the header's feature mask.
pub(crate) fn parse_lossy_head_checked<'a, I: crate::parse::payload::HeadInput>(
    info: ImageInfo,
    color: LossyColor,
    declared: crate::parse::header::Features,
    r: &mut I,
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    crate::parse::payload::ready(parse_lossy_head_async(
        info,
        color,
        declared,
        &mut crate::parse::payload::ReadyInput(r),
        admission,
    ))
}

pub(crate) async fn parse_lossy_head_async<'a, I: crate::parse::payload::AsyncHeadInput>(
    info: ImageInfo,
    color: LossyColor,
    declared: crate::parse::header::Features,
    r: &mut I,
    admission: &impl crate::parse::limits::Admission,
) -> Result<LossyParsed<'a>, Error> {
    #[cfg(all(feature = "lab", feature = "std"))]
    use crate::trace::describe as d;
    use crate::trace::{tf, tfd};
    admission.model()?;
    let kind =
        InputKind::of(&info).ok_or(Error::Corrupt("lossy stream with unsupported pixel format"))?;
    if kind == InputKind::U32 && !cfg!(feature = "extended") && admission.decodes() {
        return Err(Error::Unsupported("32-bit samples (extended tier)"));
    }
    let xfer = transfer::Transfer::from_cicp(info.cicp.transfer_characteristics);
    let ok = match kind {
        InputKind::U8 => true,
        InputKind::U16 => color == LossyColor::YCoCg || xfer.is_some(),
        InputKind::U32 => color == LossyColor::YCoCg || xfer.is_some(),
        InputKind::F16 | InputKind::F32 => color == LossyColor::Xyb,
    };
    if !ok
        || ((info.has_custom_precision() || kind == InputKind::U32)
            && (!info.cicp.full_range || info.cicp.matrix_coefficients != 0))
        || (precision::custom_color(&info) && color == LossyColor::Xyb && xfer.is_none())
    {
        return Err(Error::Corrupt(
            "lossy color does not match the sample type or range",
        ));
    }
    let sb = tfd!(
        "stripe height",
        r,
        r.u8().await?,
        |v: &u8| d::stripe_blocks(*v)
    );
    if sb == 0 {
        return Err(Error::Corrupt("bad stripe height"));
    }
    let gcols = tfd!("column group width", r, r.varint().await?, |v: &u64| {
        if *v == 0 {
            "0 = none: one group spans the width".into()
        } else {
            alloc::format!("{v} pixels per column group")
        }
    });
    if gcols != 0 && (gcols % 8 != 0 || gcols >= info.width as u64) {
        return Err(Error::Corrupt("bad column group width"));
    }
    let gcols = gcols as usize;
    let params = tfd!(
        "parameters",
        r,
        envelope::read_async(r, color, kind, declared, admission.decodes()).await?,
        |e: &envelope::Params| {
            alloc::format!(
                "plane scales {:?}; quant scale {}, floor {}, plane multipliers {:?}, base matrix {}, \
             DC step x{:?}/64; entropy {:?}; compact restart states {}; seed selection {:?}",
                e.scale,
                e.qp.scale,
                e.qp.floor,
                e.qp.mul,
                e.qp.matrix,
                e.qp.dc,
                e.entropy,
                e.compact,
                e.seed
            )
        }
    );
    let envelope::Params {
        scale,
        mut qp,
        entropy,
        compact: compact_states,
        seed: seed_selection,
        records,
    } = params;
    if !scale.iter().all(|s| s.is_finite() && *s > 1e-6 && *s < 1e9) {
        return Err(Error::Corrupt("bad color scale"));
    }
    if !qp.valid() {
        return Err(Error::Corrupt("bad quantization parameters"));
    }
    // 16-bit XYB: normative linear -> code table for the image's transfer.
    let xfer16 = match (kind, color, xfer) {
        (InputKind::U16, LossyColor::Xyb, Some(t)) => Some((t.table(), t.linear_max_f32())),
        (InputKind::U32, LossyColor::Xyb, Some(t)) => Some((t.table(), t.linear_max_f32())),
        (InputKind::U8, LossyColor::Xyb, Some(t)) if precision::custom_color(&info) => {
            Some((t.table(), t.linear_max_f32()))
        }
        _ => None,
    };
    let tl = tf!("AC table bytes", r, r.var_usize().await?);
    let tb = crate::trace::tfba!("AC entropy tables (bit-packed)", r, tl);
    let scoped = |bit: u8| seed_selection.scoped(bit);
    let mut table_bits = BitReader::new(tb);
    let (tables, huf) = if entropy == LossyEntropy::SplitHuf {
        let ht = huf::HufTables::read(&mut table_bits, scoped(crate::seeds::SCOPE_AC))?;
        let d = ht.dec();
        let rows = huf::WidRows::new(&ht);
        (Tables::placeholder(entropy), Some((ht, d, rows)))
    } else {
        (
            Tables::read(&mut table_bits, entropy, scoped(crate::seeds::SCOPE_AC))?,
            None,
        )
    };
    // Head records (SPEC 2.8): the unit table hint, the options of the
    // DC image (exact blocks, AQ, keep blocks, 32-bit samples), the Lossy
    // 0.4 dequantization records and the channel layouts.
    let mut hint_rec: Option<Vec<u8>> = None;
    // The reservation (head record 12): its bytes, kept like the hint's,
    // and where its payload sits.
    let mut reserve_rec: Option<(Vec<u8>, u64)> = None;
    let (mut exact_on, mut aq_on, mut skip, mut dc32) = (false, false, false, false);
    let mut sb_on = false;
    let mut parts_on = false;
    let mut unit_ordinals = false;
    let mut local_ans_contexts = false;
    let mut coding_defaults = None;
    let mut dc_coverage_rec = None;
    let mut model_refs_rec = None;
    #[cfg(all(feature = "lab", test))]
    let mut catalog_lab = None;
    #[cfg(feature = "lab")]
    let mut small_int = false;
    #[cfg(not(feature = "lab"))]
    let small_int = false;
    let mut layouts = [LossyChannelLayout::default(); 3];
    let mut layouts_rec = false;
    if records {
        use crate::parse::header::Feature;
        let n = tf!("head record bytes", r, r.var_usize().await?);
        let list_at = r.pos();
        let list = r.bytes(n).await?;
        #[cfg(all(feature = "lab", feature = "std"))]
        crate::trace::head_records(list, true);
        crate::parse::records::head_records_at(list, |id, rec, at| {
            // Options with no payload.
            let flag = |on: &mut bool| {
                if !rec.is_empty() {
                    return Err(Error::Corrupt("lossy head record length"));
                }
                *on = true;
                Ok(())
            };
            match id {
                #[cfg(all(feature = "lab", test))]
                model_candidates::HEAD => {
                    if local_ans_contexts || compact_states {
                        return Err(Error::Corrupt("conflicting lab model declarations"));
                    }
                    catalog_lab = Some(model_candidates::Catalog::read(
                        rec,
                        entropy,
                        scoped(crate::seeds::SCOPE_AC),
                        admission,
                    )?);
                    Ok(())
                }
                model_refs::HEAD => {
                    if !declared.allows(Feature::ModelReferences) {
                        return Err(Error::Corrupt("model references without feature"));
                    }
                    if local_ans_contexts || compact_states {
                        return Err(Error::Corrupt("conflicting model declarations"));
                    }
                    admission.hint(rec.len())?;
                    admission.wire_extra_bytes(rec.len() as u128)?;
                    let mut saved = Vec::new();
                    saved
                        .try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    saved.extend_from_slice(rec);
                    model_refs_rec = Some(saved);
                    Ok(())
                }
                dc_coverage::HEAD => {
                    if !declared.allows(Feature::DcCoverage) {
                        return Err(Error::Corrupt("DC coverage without feature"));
                    }
                    admission.hint(rec.len())?;
                    admission.wire_extra_bytes(rec.len() as u128)?;
                    let mut saved = Vec::new();
                    saved
                        .try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    saved.extend_from_slice(rec);
                    dc_coverage_rec = Some(saved);
                    Ok(())
                }
                coding_defaults::HEAD => {
                    if !declared.allows(Feature::CodingDefaults) {
                        return Err(Error::Corrupt("coding defaults without feature"));
                    }
                    coding_defaults = Some(coding_defaults::Defaults::read(rec, admission)?);
                    Ok(())
                }
                local_ans::HEAD => {
                    if !declared.allows(Feature::LocalAnsContexts) {
                        return Err(Error::Corrupt("local ANS context without its feature"));
                    }
                    if entropy != LossyEntropy::RunLevel || compact_states {
                        return Err(Error::Corrupt(
                            "local ANS needs RunLevel and per-unit restart states",
                        ));
                    }
                    flag(&mut local_ans_contexts)
                }
                crate::parse::records::H_UNIT_TABLE => {
                    // Its entry count needs the DC image's stripes: kept
                    // (charged) and decoded after the nested heads.
                    admission.hint(rec.len())?;
                    let mut b = Vec::new();
                    b.try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    b.extend_from_slice(rec);
                    hint_rec = Some(b);
                    Ok(())
                }
                crate::parse::records::H_UNIT_RESERVE => {
                    if hint_rec.is_some() {
                        return Err(Error::Corrupt("unit table twice"));
                    }
                    admission.hint(rec.len())?;
                    let mut b = Vec::new();
                    b.try_reserve_exact(rec.len())
                        .map_err(|_| Error::TooLarge)?;
                    b.extend_from_slice(rec);
                    reserve_rec = Some((b, list_at + at as u64));
                    Ok(())
                }
                H_EXACT if !declared.allows(Feature::ExactBlocks) => Err(Error::Corrupt(
                    "exact blocks without the feature exact_blocks",
                )),
                H_EXACT => flag(&mut exact_on),
                H_AQ => flag(&mut aq_on),
                H_KEEP => flag(&mut skip),
                // Gated syntax (SPEC 0.1): the i32 DC image needs
                // `samples_32`, declared or implied by a 32-bit sample type.
                H_DC32 if !declared.allows(Feature::Samples32) => {
                    Err(Error::Corrupt("32-bit DC without the feature samples_32"))
                }
                H_DC32 => flag(&mut dc32),
                H_EXPLICIT | H_BIAS | H_TRANSFORM => {
                    let f = match id {
                        H_EXPLICIT => Feature::ExplicitTables,
                        H_BIAS => Feature::ReconBias,
                        _ => Feature::ColorTransform,
                    };
                    // Gated syntax: a record in a file without its
                    // feature is Corrupt (a decoder without the feature
                    // already failed at the header). A structure parse
                    // (writers) skips the payload; a decoding parse reads
                    // it.
                    if !declared.allows(f) {
                        return Err(Error::Corrupt("lossy head record without its feature"));
                    }
                    if !admission.decodes() {
                        return Ok(());
                    }
                    let mut rr = ByteReader::new(rec);
                    match id {
                        H_EXPLICIT => {
                            if qp.matrix != MATRIX_EXPLICIT || qp.explicit.is_some() {
                                return Err(Error::Corrupt("explicit tables without matrix 63"));
                            }
                            qp.explicit = Some(envelope::read_explicit(&mut rr)?);
                        }
                        H_BIAS => {
                            if qp.bias.is_some() {
                                return Err(Error::Corrupt("repeated reconstruction bias"));
                            }
                            qp.bias = Some(envelope::read_bias(&mut rr)?);
                        }
                        _ => {
                            if qp.transform.is_some() {
                                return Err(Error::Corrupt("repeated color transform"));
                            }
                            qp.transform = Some(envelope::read_transform(&mut rr)?);
                        }
                    }
                    if rr.remaining() != 0 {
                        return Err(Error::Corrupt("lossy head record length"));
                    }
                    Ok(())
                }
                H_LAYOUTS if !declared.allows(Feature::ChannelLayouts) => Err(Error::Corrupt(
                    "channel layouts without the feature channel_layouts",
                )),
                H_LAYOUTS => {
                    if rec.len() != 3 {
                        return Err(Error::Corrupt("lossy head record length"));
                    }
                    for (l, &b) in layouts.iter_mut().zip(rec) {
                        *l = LossyChannelLayout::from_byte(b)?;
                    }
                    layouts_rec = true;
                    Ok(())
                }
                superblock::H_SUPERBLOCKS if !declared.allows(Feature::Superblocks) => Err(
                    Error::Corrupt("superblocks without the feature superblocks"),
                ),
                superblock::H_SUPERBLOCKS => flag(&mut sb_on),
                small::H_PARTITIONS if !declared.allows(Feature::SmallTransforms) => Err(
                    Error::Corrupt("small transforms without the feature small_transforms"),
                ),
                // LAB: a one-byte payload 1 selects the integer candidate
                // inverse (`small_lab.rs`); production decoders take only
                // the empty record.
                #[cfg(feature = "lab")]
                small::H_PARTITIONS if rec == [1] => {
                    small_int = true;
                    parts_on = true;
                    Ok(())
                }
                small::H_PARTITIONS => flag(&mut parts_on),
                crate::parse::records::H_UNIT_ORDINALS
                    if !declared.allows(Feature::UnitOrdinals) =>
                {
                    Err(Error::Corrupt(
                        "unit ordinals without the feature unit_ordinals",
                    ))
                }
                crate::parse::records::H_UNIT_ORDINALS => flag(&mut unit_ordinals),
                _ if id & 1 == 0 => Ok(()),
                _ => Err(Error::Unsupported("unknown lossy head record")),
            }
        })?;
    }
    // Lossy 0.4: matrix 63 carries its tables in record 13, and only the
    // YCoCg code-value family may carry a color transform (U8 samples only).
    if qp.matrix == MATRIX_EXPLICIT && qp.explicit.is_none() {
        return Err(Error::Corrupt("explicit tables without their record"));
    }
    if qp.transform.is_some() && color != LossyColor::YCoCg {
        return Err(Error::Corrupt("color transform outside the YCoCg family"));
    }
    if qp.transform.is_some() && (kind != InputKind::U8 || info.has_custom_precision()) {
        return Err(Error::Corrupt("color transform needs U8 samples"));
    }
    // The records carry values (explicit tables) the first check could not
    // see; the step bound must hold for the tables actually used.
    if !qp.valid() {
        return Err(Error::Corrupt("bad quantization parameters"));
    }
    admission.lossy(&info, sb, gcols, color, &layouts)?;
    if local_ans_contexts {
        let groups = if gcols == 0 {
            1
        } else {
            u128::from(info.width).div_ceil(gcols as u128)
        };
        let units = u128::from(info.height).div_ceil(8).div_ceil(u128::from(sb)) * groups;
        admission.local_ans(units)?;
    }
    let steps = qp.steps();
    let dq = [0, 1, 2].map(|c| dct::dequant_factors(&steps[c]));
    let g = Geom::new(info.width, info.height, sb as u32);
    if skip && info.channels != Channels::Rgb {
        return Err(Error::Corrupt("keep blocks outside RGB"));
    }
    // Merged blocks need the default 8x8 layouts and still frames: record 21
    // with record 9 or with record 27 is `Corrupt`, whatever the layout bytes
    // (SPEC 2.9). (Entropy coding is 0 or 3 here: 1 and 2 are retired,
    // `Unsupported` above.)
    if sb_on && (skip || layouts_rec) {
        return Err(Error::Corrupt(
            "superblocks with keep blocks or channel layouts",
        ));
    }
    // The joint map (record 23) replaces record 21's and excludes the lab
    // channel layouts (SPEC 2.11.1); keep blocks are bypassed parents.
    if parts_on && (sb_on || layouts_rec) {
        return Err(Error::Corrupt(
            "small transforms with superblocks or channel layouts",
        ));
    }
    if coding_defaults.is_some() && (sb_on || parts_on || layouts_rec) {
        return Err(Error::Corrupt("conflicting coding default maps"));
    }
    // A fourth DC-image channel carries per-block codes: AQ step
    // multipliers a / 64, and with keep blocks, 0 = keep the reference
    // block (inter frames); with exact blocks (no AQ, no keep), 1 = exact
    // block, else 0.
    let aq = aq_on || skip || exact_on;
    let dc_info = ImageInfo::new(
        g.bw as u32,
        g.bh as u32,
        if aq { Channels::Rgba } else { Channels::Rgb },
        if dc32 {
            SampleType::I32
        } else {
            SampleType::U16
        },
    );
    // Nested images group the same columns (the DC image in blocks).
    let head = async |r: &mut I,
                      info: ImageInfo,
                      want_gcols: usize,
                      scope: u8|
           -> Result<crate::v1::Parsed<'a>, Error> {
        crate::trace::trace_enter_addr("head", "2", "nested lossless image head", r.mark());
        let n = tf!("head bytes", r, r.var_usize().await?);
        let mut hr = ByteReader::new(r.bytes(n).await?);
        // Nested heads never carry a unit table of their own (SPEC 2.8).
        let p = crate::v1::parse_head_nested(info, &mut hr, admission, scoped(scope), declared)?;
        if hr.remaining() != 0 {
            return Err(Error::Corrupt("nested image head has trailing bytes"));
        }
        crate::trace::exit_addr(r.mark());
        if p.gcols != want_gcols {
            return Err(Error::Corrupt("nested image column groups"));
        }
        Ok(p)
    };
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let pl: [&str; 3] = match color {
            LossyColor::Xyb => ["Y", "X", "B-Y"],
            LossyColor::YCoCg => ["Y", "Co", "Cg"],
        };
        let off = if dc32 { "" } else { " +32768" };
        let mut n: Vec<alloc::string::String> =
            pl.iter().map(|p| alloc::format!("{p} DC{off}")).collect();
        n.push("AQ step a/64".into());
        crate::trace::set_names(Some(n));
    }
    if seed_selection.scope & crate::seeds::SCOPE_ALPHA != 0 && !info.channels.has_alpha() {
        return Err(Error::Corrupt("seeded alpha tables without alpha"));
    }
    let mut dc = head(r, dc_info, gcols / 8, crate::seeds::SCOPE_DC).await?;
    let dc_coverage = if let Some(rec) = dc_coverage_rec.as_deref() {
        let ng = if gcols == 0 { 1 } else { g.w.div_ceil(gcols) };
        let cover = dc_coverage::Coverage::read(rec, g.stripes(), ng, g.sb, g.bh, admission)?;
        if cover.rectangles.iter().any(|&(a, b, _)| b - a > dc.hs) {
            return Err(Error::Corrupt("DC coverage exceeds admitted rows"));
        }
        dc.unit_rectangles = Some(alloc::sync::Arc::clone(&cover.rectangles));
        Some(alloc::sync::Arc::new(cover))
    } else {
        None
    };
    #[cfg(all(feature = "lab", feature = "std"))]
    crate::trace::set_names(Some(alloc::vec!["alpha".into()]));
    admission.dc(&dc)?;
    if dc.hs % g.sb != 0 {
        return Err(Error::Corrupt(
            "DC stripes do not cover whole lossy stripes",
        ));
    }
    let alpha = if info.channels.has_alpha() {
        let ai = ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
            .with_significant_bits(&[info.channel_bits(info.channels.count() - 1).unwrap()])?;
        let a = head(r, ai, gcols, crate::seeds::SCOPE_ALPHA).await?;
        if a.hs != g.sb * 8 {
            return Err(Error::Corrupt("alpha stripes do not match"));
        }
        Some(a)
    } else {
        None
    };
    // Exact blocks: varint(len) + [varint B (exact blocks per packed row),
    // varint exact blocks, the nested RGB8 image's lossless head].
    let exact = if exact_on {
        if !matches!(info.channels, Channels::Rgb | Channels::Rgba)
            || info.sample != SampleType::U8
            || aq_on
            || skip
        {
            return Err(Error::Corrupt(
                "exact blocks need RGB(A)8 without AQ or keep blocks",
            ));
        }
        let n = r.var_usize().await?;
        let mut er = ByteReader::new(r.bytes(n).await?);
        // Units in decode order (stripe-major, then column group) and their
        // blocks.
        let gb = if gcols == 0 { g.bw } else { gcols / 8 };
        let ng = g.bw.div_ceil(gb);
        let units = g.stripes().saturating_mul(ng);
        let unit_blocks = |u: usize| {
            let (by0, by1) = g.rows(u / ng);
            let gi = u % ng;
            (by1 - by0) * (gb.min(g.bw - gi * gb))
        };
        Some(exact::Exact::parse_head(
            &mut er,
            g.bw,
            units,
            unit_blocks,
            admission,
        )?)
    } else {
        None
    };
    #[cfg(all(feature = "lab", feature = "std"))]
    crate::trace::set_names(None);
    let rans = huf.is_none().then(|| RansTables::new(&tables));
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let planes = match color {
            LossyColor::Xyb => "Y, X, B - Y",
            LossyColor::YCoCg => "Y, Co, Cg",
        };
        crate::trace::note(
            "internal color",
            alloc::format!(
                "{color:?} planes {planes}: the {:?} {:?} input is converted to three f32 planes \
                 before the transform; the decoder converts back to {:?} {:?} on output{}",
                info.channels,
                info.sample,
                info.channels,
                info.sample,
                if xfer16.is_some() {
                    " (through the transfer table for 16-bit XYB)"
                } else {
                    ""
                }
            ),
        );
        crate::trace::note(
            "coefficients",
            alloc::format!(
                "per plane {:?}; quantized to i32 (|q| <= 2^22 - 1); stripes of {} pixel rows; \
                 entropy {entropy:?}",
                layouts.map(|l| alloc::format!("{:?} {:?}", l.transform(), l.sampling())),
                8 * sb as u32
            ),
        );
        crate::trace::note(
            "DC image",
            alloc::format!(
                "nested lossless image, {}x{} blocks, {} channels of {:?}: per-block DC of each plane{}",
                g.bw,
                g.bh,
                if aq { 4 } else { 3 },
                dc.info.sample,
                if aq {
                    ", plus the AQ step multiplier a/64"
                } else {
                    ""
                }
            ),
        );
        if let Some(a) = &alpha {
            crate::trace::note(
                "alpha",
                alloc::format!(
                    "nested lossless image, full resolution, {:?}: alpha is exact, never transformed",
                    a.info.sample
                ),
            );
        }
    }
    let model_refs = model_refs_rec
        .as_deref()
        .map(|rec| {
            model_refs::Models::read(
                rec,
                &dc,
                entropy,
                declared,
                g.stripes()
                    .checked_mul(if gcols == 0 { 1 } else { g.w.div_ceil(gcols) })
                    .ok_or(Error::TooLarge)?,
                admission,
            )
        })
        .transpose()?;
    let default_maps = coding_defaults.is_some();
    let mut p = LossyParsed {
        info,
        g,
        color,
        inv_scale: scale.map(|s| 1.0 / s),
        native_quantizer: qp.matrix == MATRIX_EXPLICIT
            && qp.scale == 1.0
            && qp.floor == 1.0
            && qp.mul == [1.0; 3]
            && qp.dc == [64; 3]
            && qp.bias.is_none()
            && qp.transform.is_none()
            && qp.explicit.is_some_and(|tables| {
                tables
                    .iter()
                    .flatten()
                    .all(|&v| (1.0..=65535.0).contains(&v) && v == v as u16 as f32)
            }),
        dq,
        bias: qp
            .bias
            .filter(|b| !b.is_zero())
            .map(|b| alloc::boxed::Box::new(b.tables())),
        transform: qp
            .transform
            .and_then(|t| t.decode())
            .map(alloc::boxed::Box::new),
        layouts,
        layout_steps: core::array::from_fn(|c| {
            [false, true].map(|split| {
                let l = layouts[c].layout(split);
                if l == subblock::Layout::FULL8 {
                    steps[c]
                } else {
                    l.steps(&steps[c])
                }
            })
        }),
        adaptive_mask: layout::adaptive_mask(&layouts),
        compact_states,
        local_ans_contexts,
        coding_defaults,
        model_refs,
        dc_coverage,
        uniform_dc_k: dc.hs / g.sb,
        #[cfg(all(feature = "lab", test))]
        catalog_lab,
        tables,
        huf,
        rans,
        dc: Some(dc),
        aq,
        xfer16,
        alpha,
        gcols,
        skip,
        hint: None,
        reserve: None,
        unit_ordinals,
        stripes: Vec::new(),
        payload_len: 0,
        exact,
        // Record 23 merges only in files that declare `superblocks`.
        sb: (sb_on
            || ((parts_on || default_maps)
                && declared.allows(crate::parse::header::Feature::Superblocks)))
        .then(|| alloc::boxed::Box::new(superblock::Tables::new(&steps))),
        map: if parts_on {
            MapKind::Joint
        } else if sb_on {
            MapKind::Merges
        } else {
            MapKind::None
        },
        small: (parts_on
            || (default_maps && declared.allows(crate::parse::header::Feature::SmallTransforms)))
        .then(|| {
            alloc::boxed::Box::new(steps.map(|t| {
                #[cfg(feature = "lab")]
                if small_int {
                    return small_lab::dequant4_int(&t);
                }
                small::dequant4(&t)
            }))
        }),
        small_int,
    };
    // One hint entry per main record.
    let records = |p: &LossyParsed<'_>| {
        p.unit_map()
            .count(p.g.stripes())
            .map_err(|_| Error::Corrupt("unit table length"))
    };
    if let Some(b) = hint_rec {
        let n = records(&p)?;
        p.hint = Some(crate::parse::records::hint_entries(&b, n)?);
    } else if let Some((b, at)) = reserve_rec {
        let n = records(&p)?;
        let (state, filled) = crate::parse::records::reserve_entries(&b, n)?;
        p.hint = filled;
        p.reserve = Some(crate::parse::records::Reserve {
            state,
            at,
            len: b.len(),
        });
    }
    Ok(p)
}

/// LAB trace: labels the records of group `gi` in stripe `s` (DC when a DC
/// stripe starts, then alpha, then AC) with their image regions.
#[cfg(all(feature = "lab", feature = "std"))]
#[allow(clippy::too_many_arguments)]
pub(in crate::lossy) fn trace_lossy_group(
    g: &Geom,
    s: usize,
    k: usize,
    gi: usize,
    x0: usize,
    gw: usize,
    alpha: bool,
    color: LossyColor,
    aq: bool,
) {
    if !crate::trace::on() {
        return;
    }
    let planes = match color {
        LossyColor::Xyb => "Y, X, B-Y",
        LossyColor::YCoCg => "Y, Co, Cg",
    };
    let (by0, by1) = g.rows(s);
    let y0 = (by0 * 8) as u64;
    let rows = ((by1 * 8).min(g.h) - by0 * 8) as u64;
    let gw = gw.min(g.w - x0) as u64;
    let x0 = x0 as u64;
    if s.is_multiple_of(k) {
        let last = (s + k).min(g.stripes()) - 1;
        let y1 = (g.rows(last).1 * 8).min(g.h) as u64;
        crate::trace::tnext!(
            alloc::format!(
                "DC stripe {} group {gi} ({planes} DC{}, one value per 8x8 block)",
                s / k,
                if aq { ", AQ step" } else { "" }
            ),
            Some([x0, y0, gw, y1 - y0])
        );
    }
    if alpha {
        crate::trace::tnext!(
            alloc::format!("alpha stripe {s} group {gi} (alpha, exact)"),
            Some([x0, y0, gw, rows])
        );
    }
    crate::trace::tnext!(
        alloc::format!("AC stripe {s} group {gi} (planes {planes}: DCT coefficients)"),
        Some([x0, y0, gw, rows])
    );
}

/// Bytes of a lossy `DATA` payload.
pub(crate) fn payload_len_checked(
    info: &ImageInfo,
    version: u8,
    color: LossyColor,
    lossy: crate::parse::header::Features,
    body: &[u8],
    admission: &impl crate::parse::limits::Admission,
) -> Result<usize, Error> {
    Ok(
        parse_lossy_prefix_checked(info.clone(), version, color, lossy, body, admission)?
            .payload_len,
    )
}
