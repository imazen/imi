//! Encoded-domain settings conversion. Inverse transforms are never involved.

use super::super::*;
use crate::put_varint;
use alloc::vec::Vec;

/// Fully resolved transform cell; independent of every preceding cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cell {
    partition: u8,
    plans: [u8; 12],
    ac: [u8; 4],
}

fn delta(out: &mut Vec<u8>, base: &Cell, cell: &Cell) {
    let edits = base
        .plans
        .iter()
        .zip(cell.plans)
        .enumerate()
        .fold(0u64, |mask, (i, (&a, b))| mask | (u64::from(a != b) << i));
    let mask = u64::from(base.partition != cell.partition)
        | (u64::from(edits != 0) << 1)
        | (u64::from(base.ac != cell.ac) << 2);
    put_varint(out, mask);
    if mask & 1 != 0 {
        put_varint(out, u64::from(cell.partition));
    }
    if mask & 2 != 0 {
        put_varint(out, edits);
        for (i, &plan) in cell.plans.iter().enumerate() {
            if edits & (1 << i) != 0 {
                put_varint(out, u64::from(plan));
            }
        }
    }
    if mask & 4 != 0 {
        if cell.ac.iter().all(|&value| value == cell.ac[0]) {
            put_varint(out, u64::from(cell.ac[0]));
        } else {
            put_varint(out, 256);
            for &value in &cell.ac {
                put_varint(out, u64::from(value));
            }
        }
    }
}

fn encode(cells: &[Cell], base: &Cell, rows: usize, cols: usize) -> Result<Vec<u8>, Error> {
    let Some(group) = cells.first() else {
        return Err(Error::Corrupt("empty V4 transform cell grid"));
    };
    if cells.iter().all(|c| c == base) {
        return Ok(Vec::new());
    }
    let mut best = encode_settings(cells, base, group, None)?;
    if !cells.iter().all(|c| c.partition == group.partition) {
        let map = superblock::map::encode_v4_partitions(rows, cols, |i| cells[i].partition)?;
        let group = Cell {
            partition: base.partition,
            ..*group
        };
        let dense = encode_settings(cells, base, &group, Some(&map))?;
        if dense.len() < best.len() {
            best = dense;
        }
    }
    #[cfg(feature = "encode")]
    if *base == Cell::default()
        && cells.iter().any(|cell| cell.plans.contains(&1))
        && cells
            .iter()
            .all(|cell| cell.ac == [0; 4] && cell.plans.iter().all(|&p| p <= 1))
    {
        let columns = cols.div_ceil(2);
        let map = crate::lossy::joint_settings::encode(
            rows,
            cols,
            |i| cells[i].partition,
            |y, x| {
                let cell = &cells[y / 2 * columns + x / 2];
                let parent = 2 * (y & 1) + (x & 1);
                let planes = &cell.plans[3 * parent..][..3];
                planes[0] | planes[1] << 1 | planes[2] << 2
            },
        )?;
        if let Some(map) = map
            && map.len() < best.len().saturating_sub(1)
        {
            let capacity = map.len().checked_add(1).ok_or(Error::TooLarge)?;
            let mut joint = Vec::new();
            joint
                .try_reserve_exact(capacity)
                .map_err(|_| Error::TooLarge)?;
            joint.push(crate::v4::JOINT_SETTINGS);
            joint.extend_from_slice(&map);
            return Ok(joint);
        }
    }
    Ok(best)
}

fn encode_settings(
    cells: &[Cell],
    base: &Cell,
    group: &Cell,
    map: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    let inherit = |cell: &Cell| Cell {
        partition: if map.is_some() {
            cell.partition
        } else {
            group.partition
        },
        ..*group
    };
    let count = cells.iter().filter(|c| **c != inherit(c)).count();
    let capacity = count
        .checked_mul(48)
        .and_then(|n| n.checked_add(map.map_or(0, <[u8]>::len)))
        .and_then(|n| n.checked_add(64))
        .ok_or(Error::TooLarge)?;
    let mut out = Vec::new();
    out.try_reserve_exact(capacity)
        .map_err(|_| Error::TooLarge)?;
    if map.is_some() {
        out.push(crate::v4::RANGE_PARTITIONS);
    }
    if group == base {
        put_varint(&mut out, 0);
    } else {
        put_varint(&mut out, 1);
        delta(&mut out, base, group);
    }
    if let Some(map) = map {
        blob(&mut out, map);
    }
    put_varint(&mut out, count as u64);
    let mut next = 0;
    for (i, cell) in cells.iter().enumerate() {
        let inherited = inherit(cell);
        if *cell == inherited {
            continue;
        }
        put_varint(&mut out, (i - next) as u64);
        put_varint(&mut out, 1);
        delta(&mut out, &inherited, cell);
        next = i + 1;
    }
    Ok(out)
}

fn fixed_plans(p: &LossyParsed<'_>) -> Cell {
    fixed_layout_plans(&p.layouts)
}
fn fixed_layout_plans(layouts: &[LossyChannelLayout; 3]) -> Cell {
    let mut cell = Cell::default();
    for parent in cell.plans.as_chunks_mut::<3>().0 {
        for (c, plan) in parent.iter_mut().enumerate() {
            let layout = layouts[c].layout(false);
            *plan = if layout == subblock::Layout::FULL8 {
                0
            } else {
                2 + layout.byte()
            };
        }
    }
    cell
}

/// Serialize already-selected encoder decisions. This boundary neither reads
/// compressed AC tokens nor expands a decoder model. The existing band encoder
/// supplies its final coefficient/partition view before releasing that band.
#[cfg(all(feature = "encode", feature = "std"))]
pub(in crate::lossy) fn encoder_settings(
    stripe: &StripeCoefs,
    layouts: &[LossyChannelLayout; 3],
    partitioned: bool,
) -> Result<Vec<u8>, Error> {
    let base = fixed_layout_plans(layouts);
    if !partitioned && stripe.splits.is_empty() && !layouts.iter().any(|layout| layout.adaptive()) {
        return Ok(Vec::new());
    }

    let count = stripe
        .nby
        .div_ceil(2)
        .checked_mul(stripe.bw.div_ceil(2))
        .ok_or(Error::TooLarge)?;
    let mut cells = Vec::new();
    cells
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    // Native joint maps omit a split where the final channel has no AC.
    let ac = stripe
        .splits
        .iter()
        .any(|&bits| bits != 0)
        .then(|| superblock::ac_bits(&stripe.q, stripe.nby * stripe.bw));
    for cy in (0..stripe.nby).step_by(2) {
        for cx in (0..stripe.bw).step_by(2) {
            let mut cell = base;
            let mut merge = [0; 4];
            for y in 0..2.min(stripe.nby - cy) {
                for x in 0..2.min(stripe.bw - cx) {
                    let j = y * 2 + x;
                    let b = (cy + y) * stripe.bw + cx + x;
                    let mode = stripe.modes.get(b).copied().unwrap_or(0);
                    if partitioned {
                        merge[j] = mode;
                    }
                    for (c, layout) in layouts.iter().enumerate() {
                        let split = stripe.splits.get(b).copied().unwrap_or(0)
                            & ac.as_ref().map_or(0, |values| values[b])
                            & (1 << c)
                            != 0;
                        let shape = layout.layout(!partitioned && mode & (1 << c) != 0);
                        cell.plans[j * 3 + c] = if split {
                            1
                        } else if shape == subblock::Layout::FULL8 {
                            0
                        } else {
                            2 + shape.byte()
                        };
                    }
                }
            }
            cell.partition =
                superblock::symbol_of(merge).ok_or(Error::InvalidInput("V4 encoder partition"))?;
            cells.push(cell);
        }
    }
    encode(&cells, &base, stripe.nby, stripe.bw)
}

#[cfg(all(feature = "encode", feature = "std"))]
pub(in crate::lossy) fn encoder_image_settings(layouts: &[LossyChannelLayout; 3]) -> Vec<u8> {
    let mut output = Vec::new();
    number(&mut output, 0);
    delta(&mut output, &Cell::default(), &fixed_layout_plans(layouts));
    output
}

fn legacy_delta(out: &mut Vec<u8>, d: coding_defaults::Delta) {
    number(
        out,
        u64::from(d.partition.is_some())
            | (u64::from(d.small.is_some()) << 1)
            | (u64::from(d.scale.is_some()) << 2),
    );
    if let Some(partition) = d.partition {
        number(out, u64::from(partition));
    }
    if let Some(small) = d.small {
        // A legacy small mask replaces all twelve plans, including zeroes.
        // Reusable deltas must retain clears under every legal parent scope.
        number(out, 4095);
        for i in 0..12 {
            number(out, u64::from(small >> i & 1));
        }
    }
    if let Some(scale) = d.scale {
        if scale.iter().all(|&v| v == scale[0]) {
            number(out, u64::from(scale[0]));
        } else {
            number(out, 256);
            for v in scale {
                number(out, u64::from(v));
            }
        }
    }
}

fn settings_capacity(p: &LossyParsed<'_>) -> Result<usize, Error> {
    p.coding_defaults
        .as_ref()
        .map_or(0, |d| d.sets().len())
        .checked_mul(36)
        .and_then(|n| n.checked_add(64))
        .ok_or(Error::TooLarge)
}
fn image_settings(p: &LossyParsed<'_>) -> Result<(Vec<u8>, Cell), Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(settings_capacity(p)?)
        .map_err(|_| Error::TooLarge)?;
    let base = if let Some(defaults) = &p.coding_defaults {
        number(&mut out, defaults.sets().len() as u64);
        for &(id, d) in defaults.sets() {
            number(&mut out, id);
            legacy_delta(&mut out, d);
        }
        let state = defaults.image();
        let mut base = Cell {
            partition: state.partition,
            ac: state.scale,
            ..Cell::default()
        };
        for (i, plan) in base.plans.iter_mut().enumerate() {
            *plan = (state.small >> i & 1) as u8;
        }
        base
    } else {
        number(&mut out, 0);
        fixed_plans(p)
    };
    delta(&mut out, &Cell::default(), &base);
    Ok((out, base))
}

fn scoped_settings(
    p: &LossyParsed<'_>,
    s: usize,
    gi: usize,
    bytes: &[u8],
) -> Result<Vec<u8>, Error> {
    let defaults = p
        .coding_defaults
        .as_ref()
        .ok_or(Error::Corrupt("missing native scoped defaults"))?;
    let (settings, _) = coding_defaults::Defaults::split(bytes)?;
    if settings.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = ByteReader::new(settings);
    let mut out = Vec::new();
    out.try_reserve_exact(settings.len().checked_mul(16).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    fn selection(
        r: &mut ByteReader<'_>,
        out: &mut Vec<u8>,
        defaults: &coding_defaults::Defaults,
    ) -> Result<(), Error> {
        match r.varint()? {
            0 => number(out, 0),
            1 => {
                let d = coding_defaults::read_delta(r)?;
                if d == coding_defaults::Delta::default() {
                    number(out, 0);
                } else {
                    number(out, 1);
                    legacy_delta(out, d);
                }
            }
            2 => {
                let id = r.varint()?;
                if id == 0 {
                    number(out, 0);
                } else {
                    if defaults
                        .sets()
                        .binary_search_by_key(&id, |&(id, _)| id)
                        .is_err()
                    {
                        return Err(Error::Corrupt("unknown native coding set"));
                    }
                    number(out, 2);
                    number(out, id);
                }
            }
            _ => return Err(Error::Unsupported("native coding selection kind")),
        }
        Ok(())
    }
    selection(&mut r, &mut out, defaults)?;
    let count = r.var_usize()?;
    let (_, g) = p.group(gi);
    let (a, b) = g.rows(s);
    let cells = (b - a)
        .div_ceil(2)
        .checked_mul(g.bw.div_ceil(2))
        .ok_or(Error::TooLarge)?;
    if count > cells || count > r.remaining() / 2 {
        return Err(Error::Corrupt("native scoped override count"));
    }
    number(&mut out, count as u64);
    let mut next = 0usize;
    for _ in 0..count {
        let gap = r.var_usize()?;
        let at = next
            .checked_add(gap)
            .filter(|&at| at < cells)
            .ok_or(Error::Corrupt("native scoped override position"))?;
        number(&mut out, gap as u64);
        selection(&mut r, &mut out, defaults)?;
        next = at + 1;
    }
    if r.remaining() != 0 {
        return Err(Error::Corrupt("native scoped settings tail"));
    }
    if out == [0, 0] {
        out.clear();
    }
    Ok(out)
}

/// Resolve only the old unit's DC control rectangle, not an image-sized map.
pub(super) fn scales(p: &LossyParsed<'_>, s: usize, gi: usize) -> Result<Vec<f32>, Error> {
    if !p.aq {
        return Ok(Vec::new());
    }
    let dc = p.dc.as_ref().ok_or(Error::Corrupt("missing native DC"))?;
    let span = p.dc_span(s, gi);
    let bytes = dc.stripes[span.unit];
    let (selected, body) = if let Some(models) = &p.model_refs {
        models.dc(bytes, dc)?
    } else {
        (None, bytes)
    };
    let model = selected.as_deref().unwrap_or(dc);
    let om = crate::v1::OutMap::new(&model.info, &model.layout);
    let mut rows = crate::v1::UnitRows::new_with(model, span.unit, body)?;
    let (_, g) = p.group(gi);
    let (start, end) = g.rows(s);
    let row_bytes = g.bw.checked_mul(dc.bpp()).ok_or(Error::TooLarge)?;
    let mut row = Vec::new();
    row.try_reserve_exact(row_bytes)
        .map_err(|_| Error::TooLarge)?;
    row.resize(row_bytes, 0);
    let mut values = Vec::new();
    let mut scales = Vec::new();
    scales
        .try_reserve_exact((end - start).checked_mul(g.bw).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    for y in span.start * p.g.sb..end {
        rows.next_row(model, &om, &mut row)?;
        if y >= start {
            values.clear();
            dc_values(
                dc.info.sample,
                p.aq,
                p.exact.is_some(),
                &row,
                &mut values,
                &mut scales,
            )?;
        }
    }
    Ok(scales)
}

/// Flatten legacy syntax into independent V4 transform-cell settings. All
/// quantized coefficients, numeric controls and entropy bodies stay unchanged.
fn settings(
    p: &LossyParsed<'_>,
    s: usize,
    gi: usize,
    bytes: &[u8],
    base: &Cell,
) -> Result<Vec<u8>, Error> {
    let (_, g) = p.group(gi);
    let (a, b) = g.rows(s);
    let rows = b - a;
    let columns = g.bw;
    if p.coding_defaults.is_some() {
        return scoped_settings(p, s, gi, bytes);
    }
    if p.map == MapKind::None && p.adaptive_mask == 0 {
        return Ok(Vec::new());
    }
    let n = rows
        .div_ceil(2)
        .checked_mul(columns.div_ceil(2))
        .ok_or(Error::TooLarge)?;
    let mut cells = Vec::new();
    cells.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    cells.resize(
        n,
        Cell {
            plans: base.plans,
            ..Cell::default()
        },
    );
    let mut roles = Vec::new();
    let mut splits = Vec::new();
    if p.map == MapKind::Merges {
        superblock::read_map(bytes, rows, columns, &mut roles)?;
    } else if p.map == MapKind::Joint {
        let (q, eobs, _, _) =
            decode_stripe_raw_bytes(p, &g, s, bytes, Group::ALL, false, Vec::new(), Vec::new())?;
        let aq = scales(p, s, gi)?;
        superblock::read_joint_map(
            bytes,
            (rows, columns),
            (&aq, code0_skips(p)),
            (&q, &eobs),
            &mut roles,
            &mut splits,
        )?;
    }
    let (modes, _) = p.record(&g, s, bytes)?;
    for cy in 0..rows.div_ceil(2) {
        for cx in 0..columns.div_ceil(2) {
            let i = cy * columns.div_ceil(2) + cx;
            let mut merge = [0; 4];
            for y in 0..2.min(rows - cy * 2) {
                for x in 0..2.min(columns - cx * 2) {
                    let j = y * 2 + x;
                    let b = (cy * 2 + y) * columns + cx * 2 + x;
                    merge[j] = roles.get(b).copied().unwrap_or(0);
                    for c in 0..3 {
                        let layout = p.layouts[c].layout(modes.get(b) & (1 << c) != 0);
                        cells[i].plans[j * 3 + c] =
                            if splits.get(b).copied().unwrap_or(0) & (1 << c) != 0 {
                                1
                            } else if layout == subblock::Layout::FULL8 {
                                0
                            } else {
                                2 + layout.byte()
                            };
                    }
                }
            }
            cells[i].partition = superblock::symbol_of(merge)
                .ok_or(Error::Corrupt("native partition cannot be represented"))?;
        }
    }
    encode(&cells, base, rows, columns)
}

use crate::v4::emit::{Converted, PayloadWriter};
use crate::v4::{EntryKind, ObjectIntegrity, SetupContext, TileLayout, TileOrder};
use alloc::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SourceModel<'a> {
    role: u8,
    bank: u64,
    bytes: &'a [u8],
}

struct SourceModels<'a> {
    catalog: BTreeMap<u64, SourceModel<'a>>,
    introduced: BTreeMap<u64, SourceModel<'a>>,
    banks: BTreeSet<u64>,
    coder: LossyEntropy,
}

impl<'a> SourceModels<'a> {
    fn new(records: &'a [u8], coder: LossyEntropy) -> Result<Self, Error> {
        let mut result = Self {
            catalog: BTreeMap::new(),
            introduced: BTreeMap::new(),
            banks: BTreeSet::new(),
            coder,
        };
        crate::parse::records::head_records(records, |id, bytes| {
            if id != model_refs::HEAD {
                return Ok(());
            }
            let mut r = ByteReader::new(bytes);
            if r.varint()? != 1 {
                return Err(Error::Unsupported("model references version"));
            }
            let count = r.var_usize()?;
            for _ in 0..count {
                let id = r.varint()?;
                let definition = model_refs::WireDefinition::read(&mut r)?;
                let model = SourceModel {
                    role: definition.role as u8,
                    bank: definition.bank,
                    bytes: definition.bytes,
                };
                result.banks.insert(model.bank);
                result.catalog.insert(id, model);
            }
            if r.remaining() != 0 {
                return Err(Error::Corrupt("native model catalog tail"));
            }
            Ok(())
        })?;
        Ok(result)
    }
    fn scan(&mut self, role: u64, bytes: &[u8]) -> Result<(), Error> {
        let (selector, _) = model_refs::prefix(&mut ByteReader::new(bytes), role, self.coder)?;
        if let model_refs::Selector::Inline(model) | model_refs::Selector::Define(model) = selector
        {
            self.banks.insert(model.bank);
        }
        Ok(())
    }
    fn select(
        &mut self,
        role: u8,
        ordinal: u64,
        bytes: &'a [u8],
    ) -> Result<(Option<SourceModel<'a>>, bool, bool, &'a [u8]), Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, compact) = model_refs::prefix(&mut r, u64::from(role), self.coder)?;
        let transient = matches!(selector, model_refs::Selector::Inline(_));
        let model = match selector {
            model_refs::Selector::Root => None,
            model_refs::Selector::Catalog(id) => Some(
                *self
                    .catalog
                    .get(&id)
                    .ok_or(Error::Corrupt("missing native catalog model"))?,
            ),
            model_refs::Selector::Earlier(id) => Some(
                *self
                    .introduced
                    .get(&id)
                    .ok_or(Error::Corrupt("missing native first-use model"))?,
            ),
            model_refs::Selector::Inline(definition) | model_refs::Selector::Define(definition) => {
                let model = SourceModel {
                    role,
                    bank: definition.bank,
                    bytes: definition.bytes,
                };
                if !transient {
                    self.introduced.insert(ordinal, model);
                }
                Some(model)
            }
        };
        if model.is_some_and(|model| model.role != role) {
            return Err(Error::Corrupt("native model role mismatch"));
        }
        Ok((model, transient, compact, &bytes[r.pos..]))
    }
}

fn number(out: &mut Vec<u8>, value: u64) {
    crate::put_varint(out, value);
}
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    number(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn numeric(head: &[u8]) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(head);
    let fields = r.varint()?;
    if fields & !3 != 0 {
        return Err(Error::Unsupported("native nested numeric fields"));
    }
    r.u8()?;
    r.varint()?;
    r.varint()?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(head.len())
        .map_err(|_| Error::TooLarge)?;
    number(&mut result, fields);
    result.extend_from_slice(&head[r.pos..]);
    Ok(result)
}

struct OutputModels {
    // Shared source definitions borrow one stable location in this payload.
    // Comparing table contents on every reference would multiply work by the
    // table length; identity lookup only needs the typed borrowed location.
    cache: BTreeMap<(u8, u64, usize, usize), u64>,
    next: [u64; 4],
}
impl OutputModels {
    fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            next: [0; 4],
        }
    }
    fn define(
        &mut self,
        writer: &mut PayloadWriter,
        model: SourceModel<'_>,
        target: Option<u64>,
    ) -> Result<u64, Error> {
        let key = (
            model.role,
            model.bank,
            model.bytes.as_ptr() as usize,
            model.bytes.len(),
        );
        if let Some(&id) = self.cache.get(&key) {
            return Ok(id);
        }
        let id = self.next[model.role as usize];
        self.next[model.role as usize] = id.checked_add(1).ok_or(Error::TooLarge)?;
        let converted = if model.role == 1 {
            None
        } else {
            Some(numeric(model.bytes)?)
        };
        let bytes = converted.as_deref().unwrap_or(model.bytes);
        let mut body = Vec::new();
        body.try_reserve_exact(bytes.len().checked_add(51).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        for value in [u64::from(model.role), id, u64::from(target.is_some())] {
            number(&mut body, value);
        }
        if let Some(target) = target {
            number(&mut body, target);
        }
        number(&mut body, model.bank);
        blob(&mut body, bytes);
        let kind = if target.is_some() {
            EntryKind::NextModel
        } else {
            EntryKind::Model
        };
        writer.object(
            crate::registry::MODEL_DEFINITION,
            kind,
            model.role,
            id,
            &body,
        )?;
        if target.is_none() {
            self.cache.insert(key, id);
        }
        Ok(id)
    }
}

pub(in crate::lossy) fn quantizer(
    params: &envelope::Params,
    color: LossyColor,
    kind: InputKind,
    xyb_power: bool,
) -> Vec<u8> {
    let qp = &params.qp;
    let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits());
    let mask = u64::from(qp.matrix != envelope::default_matrix(color))
        | (u64::from(qp.dc != default_dc_step(color)) << 1)
        | (u64::from(qp.floor.to_bits() != 1f32.to_bits()) << 2)
        | (u64::from(!same(&params.scale, &envelope::defaults(color, kind))) << 3)
        | (u64::from(!same(
            &qp.mul,
            &QParams::from_quality(90.0, color, qp.matrix).mul,
        )) << 4)
        | (u64::from(xyb_power) * xyb_power::QUANTIZER_BIT);
    let mut bytes = Vec::new();
    number(&mut bytes, mask);
    bytes.extend_from_slice(&qp.scale.to_le_bytes());
    if mask & 1 != 0 {
        number(&mut bytes, u64::from(qp.matrix));
    }
    if mask & 2 != 0 {
        for value in qp.dc {
            number(&mut bytes, u64::from(value));
        }
    }
    if mask & 4 != 0 {
        bytes.extend_from_slice(&qp.floor.to_le_bytes());
    }
    if mask & 8 != 0 {
        for value in params.scale {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    if mask & 16 != 0 {
        for value in qp.mul {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

/// Convert an admitted native V2/V3 lossy payload without inverse transforms
/// or requantization. One spatial tile preserves the original DC/exact packing.
/// The outer rewriter owns metadata, nested files, locators and file checksums.
#[allow(clippy::too_many_arguments)]
pub(crate) fn payload<C: Fn() -> bool + Sync>(
    info: ImageInfo,
    version: u8,
    color: LossyColor,
    declared: crate::Features,
    body: &[u8],
    max_bytes: usize,
    fresh_xyb_power: bool,
    session: &crate::decode_resources::DecodeSession<C>,
) -> Result<Converted, Error> {
    if body.len() > max_bytes {
        return Err(Error::TooLarge);
    }
    let p = parse_lossy_checked(info, version, color, declared, body, &session.v4_models())?;
    if p.small_int {
        return Err(Error::Unsupported("experimental integer small transforms"));
    }
    let mut r = ByteReader::new(body);
    r.u8()?;
    r.varint()?;
    let params = envelope::read(
        &mut r,
        color,
        InputKind::of(&p.info).ok_or(Error::Unsupported("lossy sample type"))?,
        declared,
        true,
    )?;
    let length = r.var_usize()?;
    let ac = r.bytes(length)?;
    let records = if params.records {
        let length = r.var_usize()?;
        r.bytes(length)?
    } else {
        &[]
    };
    let mut retained = Vec::new();
    let mut aq_declared = false;
    crate::parse::records::head_records(records, |id, bytes| {
        aq_declared |= id == H_AQ;
        if [H_EXPLICIT, H_BIAS, H_TRANSFORM].contains(&id) || id % 2 == 0 && ![10, 12].contains(&id)
        {
            crate::parse::records::put_record(&mut retained, id, bytes);
        }
        Ok(())
    })?;
    let length = r.var_usize()?;
    let dc = r.bytes(length)?;
    let alpha = if p.alpha.is_some() {
        let length = r.var_usize()?;
        Some(r.bytes(length)?)
    } else {
        None
    };
    let exact_head = if let Some(exact) = &p.exact {
        let length = r.var_usize()?;
        let bytes = r.bytes(length)?;
        let mut er = ByteReader::new(bytes);
        er.varint()?;
        for _ in &exact.ranges {
            er.varint()?;
        }
        let start = er.pos;
        crate::v1::parse_head_nested(
            exact.img.info.clone(),
            &mut er,
            &crate::parse::limits::Unrestricted,
            None,
            crate::Features::EMPTY,
        )?;
        Some(&bytes[start..er.pos])
    } else {
        None
    };
    let mut sources = SourceModels::new(records, p.tables.mode)?;
    for scope in [
        crate::seeds::SCOPE_AC,
        crate::seeds::SCOPE_DC,
        crate::seeds::SCOPE_ALPHA,
    ] {
        if let Some(bank) = params.seed.scoped(scope) {
            sources.banks.insert(bank.id);
        }
    }
    if p.model_refs.is_some() {
        for bytes in &p.dc.as_ref().ok_or(Error::Corrupt("missing DC"))?.stripes {
            sources.scan(0, bytes)?;
        }
        for (u, bytes) in p.stripes.iter().enumerate() {
            let (_, g) = p.group(u % p.ngroups());
            sources.scan(1, p.record(&g, u / p.ngroups(), bytes)?.1)?;
        }
    }
    sources.banks.remove(&0);
    let layout = TileLayout::new(
        p.ngroups() as u32,
        p.stripe_count() as u32,
        p.ngroups() as u32,
        p.stripe_count() as u32,
        TileOrder::Raster,
    )?;
    let mut setup = Vec::new();
    setup
        .try_reserve_exact(records.len().checked_add(256).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    for value in [
        p.g.sb as u64,
        p.gcols as u64,
        p.ngroups() as u64,
        p.stripe_count() as u64,
        0,
        1,
        p.tables.mode as u64,
    ] {
        number(&mut setup, value);
    }
    blob(
        &mut setup,
        &quantizer(
            &params,
            color,
            InputKind::of(&p.info).unwrap(),
            fresh_xyb_power && !p.skip && xyb_power::supports(&p.info, color),
        ),
    );
    let control = if p.exact.is_some() {
        2
    } else if p.skip {
        if aq_declared { 4 } else { 3 }
    } else {
        u64::from(p.aq)
    };
    number(&mut setup, control);
    number(
        &mut setup,
        u64::from(p.dc.as_ref().unwrap().info.sample == SampleType::I32),
    );
    let _settings_memory = session.v4_working(settings_capacity(&p)? as u128)?;
    let (image_settings, image_default) = image_settings(&p)?;
    blob(&mut setup, &image_settings);
    let mut coverage = Vec::new();
    if p.dc_coverage.is_some() {
        number(&mut coverage, 1);
        number(&mut coverage, p.ngroups() as u64);
        for gi in 0..p.ngroups() {
            let mut spans = Vec::new();
            let mut stripe = 0;
            while stripe < p.stripe_count() {
                let span = p.dc_span(stripe, gi);
                spans.push(span.end - span.start);
                stripe = span.end;
            }
            number(&mut coverage, spans.len() as u64);
            for count in spans {
                number(&mut coverage, count as u64);
            }
        }
    } else {
        number(&mut coverage, 0);
        number(&mut coverage, p.dc_k() as u64);
    }
    blob(&mut setup, &coverage);
    number(&mut setup, sources.banks.len() as u64);
    for &bank in &sources.banks {
        number(&mut setup, bank);
    }
    number(
        &mut setup,
        1 + u64::from(alpha.is_some()) + u64::from(exact_head.is_some()),
    );
    for (role, parsed) in [(0, p.dc.as_ref()), (2, p.alpha.as_ref())] {
        if let Some(parsed) = parsed {
            number(&mut setup, role);
            number(&mut setup, parsed.lanes as u64);
        }
    }
    if let Some(exact) = &p.exact {
        for value in [
            3,
            exact.img.lanes as u64,
            exact.row_blocks() as u64,
            exact.img.hs as u64,
        ] {
            number(&mut setup, value);
        }
    }
    blob(&mut setup, &retained);
    let context =
        SetupContext::from_image_declaration(&p.info, crate::Coding::Lossy, Some(color), &setup)?;
    let mut writer =
        PayloadWriter::new(layout, ObjectIntegrity::Objects, context, &setup, max_bytes)?;
    let mut models = OutputModels::new();
    for (role, scope, head) in [
        (0, crate::seeds::SCOPE_DC, Some(dc)),
        (1, crate::seeds::SCOPE_AC, Some(ac)),
        (2, crate::seeds::SCOPE_ALPHA, alpha),
        (3, 0, exact_head),
    ] {
        if let Some(bytes) = head {
            models.define(
                &mut writer,
                SourceModel {
                    role,
                    bank: params.seed.scoped(scope).map_or(0, |bank| bank.id),
                    bytes,
                },
                None,
            )?;
        }
    }
    let mut counts = Vec::new();
    let mut plan = Vec::new();
    if let Some(exact) = &p.exact {
        for &(_, count) in &exact.ranges {
            number(&mut counts, count as u64);
        }
        blob(&mut plan, &counts);
    }
    writer.begin(0, &plan)?;
    let mut ordinal = 0;
    for (u, &bytes) in p.stripes.iter().enumerate() {
        session.adapter_work(bytes.len() as u128 + 1)?;
        let (s, gi) = (u / p.ngroups(), u % p.ngroups());
        let (_, g) = p.group(gi);
        let span = p.dc_span(s, gi);
        if span.start == s {
            let bytes = p.dc.as_ref().unwrap().stripes[span.unit];
            let (model, transient, _, entropy) = if p.model_refs.is_some() {
                sources.select(0, ordinal, bytes)?
            } else {
                (None, false, false, bytes)
            };
            let target = (s * p.ngroups() + gi) as u64;
            let selected = model.map_or(Ok(0), |model| {
                models.define(&mut writer, model, transient.then_some(target))
            })?;
            let mut body = Vec::new();
            number(&mut body, target);
            number(&mut body, selected);
            blobless(&mut body, entropy, max_bytes)?;
            writer.object(51, EntryKind::Unit, 0, target, &body)?;
            ordinal += 1;
        }
        if let Some(alpha) = &p.alpha {
            let mut body = Vec::new();
            number(&mut body, u as u64);
            number(&mut body, 0);
            blobless(&mut body, alpha.stripes[u], max_bytes)?;
            writer.object(53, EntryKind::Unit, 2, u as u64, &body)?;
            ordinal += 1;
        }
        let entropy = p.record(&g, s, bytes)?.1;
        let (model, transient, compact, entropy) = if p.model_refs.is_some() {
            sources.select(1, ordinal, entropy)?
        } else if p.local_ans_contexts {
            let (model, compact, bytes) = local_ans::split(entropy)?;
            (
                model.map(|bytes| SourceModel {
                    role: 1,
                    bank: 0,
                    bytes,
                }),
                true,
                compact,
                bytes,
            )
        } else {
            (None, false, p.compact_states, entropy)
        };
        let selected = model.map_or(Ok(0), |model| {
            models.define(&mut writer, model, transient.then_some(u as u64))
        })?;
        let needs_cells = p.map != MapKind::None || p.adaptive_mask != 0;
        let rows = g.rows(s).1 - g.rows(s).0;
        let scratch = if needs_cells {
            decoder_table_bytes()
                + crate::decoder_memory::lossy_stripe_bytes(g.w as u128, rows as u128)? * 2
                + (rows as u128).div_ceil(2) * (g.bw as u128).div_ceil(2) * 128
        } else if p.coding_defaults.is_some() {
            coding_defaults::Defaults::split(bytes)?.0.len() as u128 * 16
        } else {
            0
        };
        let _working = session.v4_working(scratch)?;
        if needs_cells {
            session.adapter_work((rows as u128) * g.bw as u128 * 192)?;
        }
        let settings = settings(&p, s, gi, bytes, &image_default)?;
        let mut body = Vec::new();
        number(&mut body, u as u64);
        blob(&mut body, &settings);
        number(&mut body, selected);
        if p.tables.mode == LossyEntropy::RunLevel {
            number(&mut body, u64::from(compact));
        }
        blobless(&mut body, entropy, max_bytes)?;
        writer.object(55, EntryKind::Unit, 1, u as u64, &body)?;
        ordinal += 1;
        if let Some(exact) = &p.exact {
            for stripe in exact.carried_by(u) {
                let at = exact.stripes_at[stripe];
                let mut body = Vec::new();
                number(&mut body, stripe as u64);
                number(&mut body, 0);
                blobless(
                    &mut body,
                    &exact.records[at.unit][at.offset..at.offset + at.len],
                    max_bytes,
                )?;
                writer.object(57, EntryKind::Unit, 3, stripe as u64, &body)?;
            }
            ordinal += 1;
        }
    }
    writer.end(0)?;
    let mut converted = writer.finish()?;
    if sources.banks.contains(&1) {
        converted.extra_features = converted.extra_features.with(crate::Feature::SeedBank1);
    }
    Ok(converted)
}

fn blobless(out: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), Error> {
    crate::v4::emit::extend(out, bytes, max)
}

#[cfg(all(test, feature = "encode"))]
mod settings_tests {
    use super::*;
    use crate::Features;
    use crate::decode_resources::{DecodePolicy, DecodeSession};

    #[test]
    fn joint_settings_roundtrip_on_odd_group() {
        let (rows, cols) = (17usize, 31usize);
        let cell_cols = cols.div_ceil(2);
        let mut cells = alloc::vec![Cell::default(); rows.div_ceil(2) * cell_cols];
        for (i, cell) in cells.iter_mut().enumerate() {
            let (y, x) = (i / cell_cols * 2, i % cell_cols * 2);
            if i.is_multiple_of(5) && y + 1 < rows && x + 1 < cols {
                cell.partition = 1;
                continue;
            }
            for dy in 0..2.min(rows - y) {
                for dx in 0..2.min(cols - x) {
                    let index = 3 * (dy * 2 + dx) + (i + dx + dy) % 3;
                    cell.plans[index] = 1;
                }
            }
        }
        let encoded = encode(&cells, &Cell::default(), rows, cols).unwrap();
        assert_eq!(encoded[0], crate::v4::JOINT_SETTINGS);
        let session = DecodeSession::new(DecodePolicy::default());
        let settings = crate::v4::LossySettings::read(&[0, 0], &session).unwrap();
        let caps = Features::EMPTY
            .with(crate::Feature::SmallTransforms)
            .with(crate::Feature::Superblocks);
        let mut visited = 0;
        settings
            .visit_unit(
                &encoded,
                rows as u32,
                cols as u32,
                caps,
                &session,
                |y, x, state| {
                    let expected = cells[y as usize * cell_cols + x as usize];
                    assert_eq!(state.partition(), expected.partition);
                    assert_eq!(state.plane_plans(), expected.plans);
                    assert_eq!(state.ac_codes(), expected.ac);
                    visited += 1;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(visited, cells.len());
    }

    #[test]
    fn range_partitions_keep_sparse_edits_and_reject_invalid_maps_before_callbacks() {
        let session = DecodeSession::new(DecodePolicy::default());
        let resolved = crate::v4::LossySettings::read(&[0, 0], &session).unwrap();
        let features = Features::EMPTY
            .with(crate::Feature::Superblocks)
            .with(crate::Feature::SmallTransforms);
        let mut cells: Vec<_> = (0..64)
            .map(|i| Cell {
                partition: (i % 8) as u8,
                ..Cell::default()
            })
            .collect();
        cells[16].plans[0] = 1;
        cells[16].ac = [32, 0, 64, 255];
        let bytes = encode(&cells, &Cell::default(), 16, 16).unwrap();
        assert_eq!(bytes[0], crate::v4::RANGE_PARTITIONS);
        for _ in 0..2 {
            resolved
                .visit_unit(&bytes, 16, 16, features, &session, |y, x, state| {
                    let cell = cells[(y * 8 + x) as usize];
                    assert_eq!(state.partition(), cell.partition);
                    assert_eq!(state.plane_plans(), cell.plans);
                    assert_eq!(state.ac_codes(), cell.ac);
                    Ok(())
                })
                .unwrap();
        }
        assert!(
            encode(&cells[..1], &Cell::default(), 2, 2)
                .unwrap()
                .is_empty()
        );
        let mut few = alloc::vec![Cell::default(); 64];
        few[10].partition = 1;
        assert_eq!(encode(&few, &Cell::default(), 16, 16).unwrap()[0], 0);

        let mut bad = Vec::new();
        for cut in 1..bytes.len() {
            bad.push((16, 16, bytes[..cut].to_vec()));
        }
        // Strict range termination, even when the map's length was changed.
        for map in [
            &[0x7f, 0xff, 0xf8][..],
            &[0x7f, 0xff, 0xf8, 0, 0],
            &[0x7f, 0xff, 0xf8, 1],
            &[0xff, 0xff, 0xff, 0xfe],
            &[0, 0, 0, 0],
        ] {
            let mut body = alloc::vec![crate::v4::RANGE_PARTITIONS, 0];
            blob(&mut body, map);
            body.push(0);
            bad.push((2, 2, body));
        }
        // A 16x16 merge declared for a horizontal-only edge cell. A sparse
        // override clearing its partition must not hide the invalid map.
        bad.push((
            1,
            2,
            alloc::vec![3, 0, 4, 0x7f, 0xff, 0xf8, 0, 1, 0, 1, 1, 0],
        ));
        for (rows, cols, body) in bad {
            assert!(
                resolved
                    .visit_unit(&body, rows, cols, features, &session, |_, _, _| panic!(
                        "invalid range map reached settings delivery"
                    ))
                    .is_err()
            );
        }
        let denied = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(63));
        assert_eq!(
            resolved.visit_unit(&bytes, 16, 16, features, &denied, |_, _, _| Ok(())),
            Err(Error::TooLarge)
        );
    }

    #[cfg(feature = "std")]
    #[test]
    #[ignore = "requires the explicit 36-file historical partition oracle"]
    fn retained_range_partitions() {
        let path =
            std::env::var_os("IMIJ_V4_RANGE_MAPS").expect("path to 36 historical map oracles");
        let mut files: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect();
        files.sort();
        assert_eq!(files.len(), 36);
        let session = DecodeSession::new(DecodePolicy::default());
        let resolved = crate::v4::LossySettings::read(&[0, 0], &session).unwrap();
        let features = Features::EMPTY.with(crate::Feature::Superblocks);
        let framed = |len| crate::varint_len(len as u64) + len;
        for path in files {
            let bytes = std::fs::read(&path).unwrap();
            let mut reader = ByteReader::new(&bytes);
            let groups = reader.u32().unwrap();
            let mut totals = [0; 5];
            for _ in 0..groups {
                let rows = usize::from(reader.u16().unwrap());
                let cols = usize::from(reader.u16().unwrap());
                let historic = reader.u32().unwrap() as usize;
                let truth = reader.bytes(rows.div_ceil(2) * cols.div_ceil(2)).unwrap();
                let cells: Vec<_> = truth
                    .iter()
                    .map(|&partition| Cell {
                        partition,
                        ..Cell::default()
                    })
                    .collect();
                let base = Cell::default();
                let body = encode(&cells, &base, rows, cols).unwrap();
                let sparse_len = if cells.iter().all(|c| c == &base) {
                    0
                } else {
                    encode_settings(&cells, &base, &cells[0], None)
                        .unwrap()
                        .len()
                };
                let mut seen = 0;
                resolved
                    .visit_unit(
                        &body,
                        rows as u32,
                        cols as u32,
                        features,
                        &session,
                        |_, _, state| {
                            assert_eq!(state.partition(), truth[seen], "{path:?} cell {seen}");
                            seen += 1;
                            Ok(())
                        },
                    )
                    .unwrap();
                assert_eq!(seen, truth.len());
                totals[0] += truth.len();
                totals[1] += historic;
                totals[2] += framed(sparse_len);
                totals[3] += framed(body.len());
                totals[4] += usize::from(body.first() == Some(&crate::v4::RANGE_PARTITIONS));
            }
            assert_eq!(reader.remaining(), 0);
            std::println!(
                "\nretained\t{}\t{groups}\t{}\t{}\t{}\t{}\t{}",
                path.file_stem().unwrap().to_str().unwrap(),
                totals[0],
                totals[1],
                totals[2],
                totals[3],
                totals[4]
            );
        }
    }

    #[test]
    fn zero_id_inheritance_and_explicit_clears_keep_native_scoped_meaning() {
        let info = ImageInfo::new(32, 16, Channels::Rgb, SampleType::U8);
        let pixels = alloc::vec![73; info.checked_buffer_len().unwrap()];
        let native = crate::ImiEncoderConfig::lossy(71.0)
            .with_column_groups(Some(32))
            .tune_lossy(|t| t.stripe_block_rows = 2)
            .encode_legacy_samples(
                crate::ImageRef::packed(&pixels, &info),
                &crate::Metadata::default(),
            )
            .unwrap();
        let mut p = parse_lossy(&native).unwrap();
        let mut head = Vec::new();
        let set_id = (1u64 << 48) + 7;
        for v in [1, 1, set_id, 6, 4095, 96, set_id] {
            number(&mut head, v);
        }
        p.coding_defaults = Some(
            coding_defaults::Defaults::read(&head, &crate::parse::limits::Unrestricted).unwrap(),
        );
        let (head, _) = image_settings(&p).unwrap();
        let session = DecodeSession::new(DecodePolicy::default());
        let resolved = crate::v4::LossySettings::read(&head, &session).unwrap();
        for values in [
            &[2, 0, 0][..],
            &[1, 0, 0],
            &[1, 1, 0, 0],
            &[1, 2, 0, 0],
            &[1, 4, 0, 0],
            &[0, 1, 0, 2, 0],
            &[2, set_id, 0],
        ] {
            let mut body = Vec::new();
            for &value in values {
                number(&mut body, value);
            }
            let mut record = Vec::new();
            blob(&mut record, &body);
            let mut expected = Vec::new();
            p.coding_defaults
                .as_ref()
                .unwrap()
                .expand(&record, 2, 4, |_, state| {
                    expected.push(state);
                    Ok(())
                })
                .unwrap();
            let encoded = scoped_settings(&p, 0, 0, &record).unwrap();
            if values == [2, 0, 0] || values == [1, 0, 0] {
                assert!(encoded.is_empty());
            }
            let mut seen = 0;
            resolved
                .visit_unit(
                    &encoded,
                    2,
                    4,
                    crate::Features::EMPTY.with(crate::Feature::SmallTransforms),
                    &session,
                    |_, _, state| {
                        assert_eq!(state.partition(), expected[seen].partition);
                        assert_eq!(state.ac_codes(), expected[seen].scale);
                        for (i, plan) in state.plane_plans().iter().enumerate() {
                            assert_eq!(*plan, (expected[seen].small >> i & 1) as u8);
                        }
                        seen += 1;
                        Ok(())
                    },
                )
                .unwrap();
            assert_eq!(seen, expected.len());
        }
    }
}
