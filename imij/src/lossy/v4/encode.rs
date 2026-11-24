//! V4 serialization of the existing band's prepared models and entropy.
//! No decoder model expansion, coefficient decode or pixel conversion occurs.
use super::super::*;
use crate::parse::records::{R_AC, R_ALPHA, R_DC, R_EXACT};
use crate::v4::emit::PayloadWriter;
use crate::v4::{EntryKind, ObjectIntegrity, SetupContext, TileLayout, TileOrder};
use alloc::{collections::BTreeMap, vec::Vec};

pub(in crate::lossy) struct ExactInput<'a> {
    pub blocks_per_row: usize,
    pub counts: &'a [usize],
    pub model: &'a [u8],
    pub lengths: &'a [u8],
    pub stripes: usize,
}
pub(in crate::lossy) struct Input<'a> {
    pub info: &'a ImageInfo,
    pub color: LossyColor,
    pub rows: usize,
    pub columns: usize,
    pub params: envelope::Params,
    pub layouts: [LossyChannelLayout; 3],
    pub dc_rows: usize,
    pub ac_head: &'a [u8],
    pub dc_head: &'a [u8],
    pub alpha_head: Option<&'a [u8]>,
    pub exact: Option<ExactInput<'a>>,
    pub control: u64,
    pub dc32: bool,
    pub scoped: bool,
    pub local_ans: bool,
    pub references: bool,
    pub partitioned: bool,
    pub retained: &'a [u8],
    pub max_bytes: usize,
}

pub(in crate::lossy) struct Encoder {
    writer: PayloadWriter,
    prefix_len: usize,
    layouts: [LossyChannelLayout; 3],
    partitioned: bool,
    scoped: bool,
    local_ans: bool,
    references: bool,
    coder: LossyEntropy,
    columns: usize,
    stripes: usize,
    next_group: u64,
    tile: u64,
    tile_rows: u64,
    source_ordinal: u64,
    next_model: [u64; 4],
    introduced: BTreeMap<u64, (u8, u64)>,
    exact_lengths: Vec<usize>,
    exact_next: usize,
    exact_pending: bool,
    exact: bool,
    max_bytes: usize,
}
fn number(out: &mut Vec<u8>, n: u64) {
    crate::put_varint(out, n);
}
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    number(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}
fn numeric(head: &[u8]) -> Result<(u8, usize, Vec<u8>), Error> {
    let mut r = ByteReader::new(head);
    let fields = r.varint()?;
    if fields & !3 != 0 {
        return Err(Error::Corrupt("bounded numeric model fields"));
    }
    let lanes = r.u8()?;
    let rows = r.var_usize()?;
    r.varint()?;
    let mut model = Vec::new();
    model
        .try_reserve_exact(head.len())
        .map_err(|_| Error::TooLarge)?;
    number(&mut model, fields);
    model.extend_from_slice(&head[r.pos..]);
    Ok((lanes, rows, model))
}
impl Encoder {
    pub(in crate::lossy) fn new(input: Input<'_>) -> Result<Self, Error> {
        let columns = if input.columns == 0 {
            1
        } else {
            (input.info.width as usize).div_ceil(input.columns)
        };
        let stripes = (input.info.height as usize)
            .div_ceil(8)
            .div_ceil(input.rows);
        let dc_k = input
            .dc_rows
            .checked_div(input.rows)
            .filter(|&k| k != 0)
            .ok_or(Error::Corrupt("bounded DC coverage"))?;
        let tile_rows = if input.exact.is_some() {
            stripes
        } else {
            dc_k.min(stripes)
        };
        let layout = TileLayout::new(
            columns as u32,
            stripes as u32,
            columns as u32,
            tile_rows as u32,
            TileOrder::Raster,
        )?;
        let mut setup = Vec::new();
        setup
            .try_reserve_exact(
                input
                    .retained
                    .len()
                    .checked_add(256)
                    .ok_or(Error::TooLarge)?,
            )
            .map_err(|_| Error::TooLarge)?;
        for value in [
            input.rows as u64,
            input.columns as u64,
            columns as u64,
            tile_rows as u64,
            0,
            2,
            input.params.entropy as u64,
        ] {
            number(&mut setup, value);
        }
        blob(
            &mut setup,
            &super::convert::quantizer(
                &input.params,
                input.color,
                InputKind::of(input.info).ok_or(Error::Unsupported("lossy sample type"))?,
                xyb_power::supports(input.info, input.color),
            ),
        );
        number(&mut setup, input.control);
        number(&mut setup, u64::from(input.dc32));
        blob(
            &mut setup,
            &super::convert::encoder_image_settings(&input.layouts),
        );
        let mut coverage = Vec::new();
        number(&mut coverage, 0);
        number(&mut coverage, dc_k as u64);
        blob(&mut setup, &coverage);
        number(&mut setup, 0); // These prepared bounded models use explicit tables.
        let (dc_lanes, _, dc) = numeric(input.dc_head)?;
        let alpha = input.alpha_head.map(numeric).transpose()?;
        let exact_model = input.exact.as_ref().map(|e| numeric(e.model)).transpose()?;
        number(
            &mut setup,
            1 + u64::from(alpha.is_some()) + u64::from(exact_model.is_some()),
        );
        for (role, lanes) in [(0, Some(dc_lanes)), (2, alpha.as_ref().map(|v| v.0))] {
            if let Some(lanes) = lanes {
                number(&mut setup, role);
                number(&mut setup, u64::from(lanes));
            }
        }
        if let (Some(exact), Some((lanes, rows, _))) = (&input.exact, &exact_model) {
            for n in [
                3,
                u64::from(*lanes),
                exact.blocks_per_row as u64,
                *rows as u64,
            ] {
                number(&mut setup, n);
            }
        }
        blob(&mut setup, input.retained);
        let prefix_len = crate::varint_len(setup.len() as u64) + setup.len();
        let context = SetupContext::from_image_declaration(
            input.info,
            crate::Coding::Lossy,
            Some(input.color),
            &setup,
        )?;
        let mut writer = PayloadWriter::new(
            layout,
            ObjectIntegrity::ObjectsAndTiles,
            context,
            &setup,
            input.max_bytes,
        )?;
        for (role, model) in [
            (0, Some(dc.as_slice())),
            (1, Some(input.ac_head)),
            (2, alpha.as_ref().map(|v| v.2.as_slice())),
            (3, exact_model.as_ref().map(|v| v.2.as_slice())),
        ] {
            if let Some(model) = model {
                let mut body = Vec::new();
                for n in [role, 0, 0, 0] {
                    number(&mut body, n);
                }
                blob(&mut body, model);
                writer.object(
                    crate::registry::MODEL_DEFINITION,
                    EntryKind::Model,
                    role as u8,
                    0,
                    &body,
                )?;
            }
        }
        let mut exact_lengths = Vec::new();
        let mut plan = Vec::new();
        if let Some(exact) = &input.exact {
            let mut counts = Vec::new();
            counts
                .try_reserve_exact(exact.counts.len().checked_mul(10).ok_or(Error::TooLarge)?)
                .map_err(|_| Error::TooLarge)?;
            for &count in exact.counts {
                number(&mut counts, count as u64);
            }
            blob(&mut plan, &counts);
            let mut r = ByteReader::new(exact.lengths);
            exact_lengths
                .try_reserve_exact(exact.stripes)
                .map_err(|_| Error::TooLarge)?;
            for _ in 0..exact.stripes {
                exact_lengths.push(r.var_usize()?);
            }
            if r.remaining() != 0 {
                return Err(Error::Corrupt("bounded exact sizes"));
            }
        }
        writer.begin(0, &plan)?;
        Ok(Self {
            writer,
            prefix_len,
            layouts: input.layouts,
            partitioned: input.partitioned,
            scoped: input.scoped,
            local_ans: input.local_ans,
            references: input.references,
            coder: input.params.entropy,
            columns,
            stripes,
            next_group: 0,
            tile: 0,
            tile_rows: tile_rows as u64,
            source_ordinal: 0,
            next_model: [1; 4],
            introduced: BTreeMap::new(),
            exact_lengths,
            exact_next: 0,
            exact_pending: false,
            exact: input.exact.is_some(),
            max_bytes: input.max_bytes,
        })
    }
    fn define(
        &mut self,
        role: u8,
        bank: u64,
        bytes: &[u8],
        transient: bool,
        target: u64,
    ) -> Result<u64, Error> {
        if bank != 0 {
            return Err(Error::Corrupt("bounded model bank"));
        }
        let id = self.next_model[role as usize];
        self.next_model[role as usize] = id.checked_add(1).ok_or(Error::TooLarge)?;
        let model = if role == 1 {
            None
        } else {
            Some(numeric(bytes)?.2)
        };
        let bytes = model.as_deref().unwrap_or(bytes);
        let mut body = Vec::new();
        for n in [u64::from(role), id, u64::from(transient)] {
            number(&mut body, n);
        }
        if transient {
            number(&mut body, target);
        }
        number(&mut body, bank);
        blob(&mut body, bytes);
        self.writer.object(
            crate::registry::MODEL_DEFINITION,
            if transient {
                EntryKind::NextModel
            } else {
                EntryKind::Model
            },
            role,
            id,
            &body,
        )?;
        Ok(id)
    }
    fn select<'a>(&mut self, role: u8, bytes: &'a [u8]) -> Result<(u64, bool, &'a [u8]), Error> {
        if self.references && matches!(role, 0 | 1) {
            let mut r = ByteReader::new(bytes);
            let (selection, compact) = model_refs::prefix(&mut r, u64::from(role), self.coder)?;
            let id = match selection {
                model_refs::Selector::Root => 0,
                model_refs::Selector::Inline(d) => {
                    self.define(role, d.bank, d.bytes, true, self.next_group)?
                }
                model_refs::Selector::Define(d) => {
                    let id = self.define(role, d.bank, d.bytes, false, self.next_group)?;
                    self.introduced.insert(self.source_ordinal, (role, id));
                    id
                }
                model_refs::Selector::Earlier(source) => {
                    let &(stored_role, id) = self
                        .introduced
                        .get(&source)
                        .ok_or(Error::Corrupt("bounded earlier model"))?;
                    if stored_role != role {
                        return Err(Error::Corrupt("bounded model role"));
                    }
                    id
                }
                model_refs::Selector::Catalog(_) => {
                    return Err(Error::Corrupt("bounded catalog model"));
                }
            };
            return Ok((id, compact, &bytes[r.pos..]));
        }
        if role == 1 && self.local_ans {
            let (model, compact, bytes) = local_ans::split(bytes)?;
            let id = model.map_or(Ok(0), |model| {
                self.define(1, 0, model, true, self.next_group)
            })?;
            return Ok((id, compact, bytes));
        }
        Ok((0, false, bytes))
    }
    pub(in crate::lossy) fn push(
        &mut self,
        id: u64,
        payload: &[u8],
        stripe: Option<&StripeCoefs>,
    ) -> Result<(), Error> {
        if id == R_EXACT {
            if !self.exact || !self.exact_pending {
                return Err(Error::Corrupt("bounded exact order"));
            }
            let mut at = 0usize;
            while at < payload.len() {
                let length = *self
                    .exact_lengths
                    .get(self.exact_next)
                    .ok_or(Error::Corrupt("bounded exact count"))?;
                let end = at
                    .checked_add(length)
                    .filter(|&n| n <= payload.len())
                    .ok_or(Error::Corrupt("bounded exact size"))?;
                let mut body = Vec::new();
                number(&mut body, self.exact_next as u64);
                number(&mut body, 0);
                crate::v4::emit::extend(&mut body, &payload[at..end], self.max_bytes)?;
                self.writer
                    .object(57, EntryKind::Unit, 3, self.exact_next as u64, &body)?;
                at = end;
                self.exact_next += 1;
            }
            self.exact_pending = false;
        } else {
            let (role, tag) = match id {
                R_DC => (0, 51),
                R_ALPHA => (2, 53),
                R_AC => (1, 55),
                _ => return Err(Error::Corrupt("bounded main record")),
            };
            let mut body = Vec::new();
            number(&mut body, self.next_group);
            let entropy = if role == 1 {
                let stripe = stripe.ok_or(Error::Corrupt("missing bounded transform choices"))?;
                let settings =
                    super::convert::encoder_settings(stripe, &self.layouts, self.partitioned)?;
                blob(&mut body, &settings);
                if self.scoped {
                    coding_defaults::Defaults::split(payload)?.1
                } else if self.partitioned {
                    superblock::skip_map(payload)?
                } else {
                    layout::read_modes(
                        payload,
                        stripe.nby * stripe.bw,
                        self.layouts
                            .iter()
                            .enumerate()
                            .fold(0, |mask, (c, l)| mask | (u8::from(l.adaptive()) << c)),
                    )?
                    .1
                }
            } else {
                payload
            };
            let (model, compact, entropy) = self.select(role, entropy)?;
            number(&mut body, model);
            if role == 1 && self.coder == LossyEntropy::RunLevel {
                number(&mut body, u64::from(compact));
            }
            crate::v4::emit::extend(&mut body, entropy, self.max_bytes)?;
            self.writer
                .object(tag, EntryKind::Unit, role, self.next_group, &body)?;
            if role == 1 {
                self.next_group = self.next_group.checked_add(1).ok_or(Error::TooLarge)?;
                self.exact_pending = self.exact;
                let total = self.columns as u64 * self.stripes as u64;
                if !self.exact
                    && self.next_group < total
                    && self
                        .next_group
                        .is_multiple_of(self.columns as u64 * self.tile_rows)
                {
                    self.writer.end(self.tile)?;
                    self.tile += 1;
                    self.writer.begin(self.tile, &[])?;
                }
            }
        }
        self.source_ordinal = self.source_ordinal.checked_add(1).ok_or(Error::TooLarge)?;
        Ok(())
    }
    pub(in crate::lossy) const fn prefix_len(&self) -> usize {
        self.prefix_len
    }
    pub(in crate::lossy) fn take_output(&mut self) -> (Vec<u8>, Vec<u8>) {
        self.writer.take_output()
    }
    pub(in crate::lossy) fn restore_output(&mut self, data: Vec<u8>, entries: Vec<u8>) {
        self.writer.restore_output(data, entries)
    }
    pub(in crate::lossy) fn finish(&mut self) -> Result<(u64, u64), Error> {
        if self.next_group != (self.columns as u64) * self.stripes as u64
            || self.exact_pending
            || self.exact_next != self.exact_lengths.len()
        {
            return Err(Error::Corrupt("incomplete bounded V4 output"));
        }
        self.writer.end(self.tile)?;
        self.writer.complete()
    }
}
