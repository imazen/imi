//! One directory schema for front/tail placement and caller-owned index scratch.

use super::{Object, ObjectIntegrity, TileExtent, TileLayout};
use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Error};

/// Directory entry class. These are index codes, not encoded record tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum EntryKind {
    Unit = 0,
    /// Payload-lifetime model definition.
    Model = 1,
    TileBegin = 2,
    TileEnd = 3,
    Ignorable = 4,
    /// Model consumed by the immediately following main unit.
    NextModel = 5,
}

impl TryFrom<u64> for EntryKind {
    type Error = Error;
    fn try_from(value: u64) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Unit),
            1 => Ok(Self::Model),
            2 => Ok(Self::TileBegin),
            3 => Ok(Self::TileEnd),
            4 => Ok(Self::Ignorable),
            5 => Ok(Self::NextModel),
            _ => Err(Error::Unsupported("V4 directory entry kind")),
        }
    }
}

impl EntryKind {
    pub(crate) const fn is_model(self) -> bool {
        matches!(self, Self::Model | Self::NextModel)
    }
    pub(crate) fn model_lifetime(lifetime: u64) -> Result<Self, Error> {
        match lifetime {
            0 => Ok(Self::Model),
            1 => Ok(Self::NextModel),
            _ => Err(Error::Unsupported("V4 model lifetime")),
        }
    }
}

/// Identity/length stored by an entry; its physical offset is a prefix sum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    /// 0 DC/numeric main, 1 AC, 2 alpha, 3 packed exact. Zero for markers/side objects.
    pub role: u8,
    /// Model/unit/tile identity, or 0-based global ignorable-object ordinal.
    pub id: u64,
    /// Complete framed byte length, including any integrity trailer.
    pub record_len: u64,
}

impl Entry {
    /// Encode into fixed caller-owned space; no image-sized index allocation.
    pub fn encode(self, out: &mut [u8; 40]) -> usize {
        let mut used = 0;
        for mut value in [
            self.kind as u64,
            u64::from(self.role),
            self.id,
            self.record_len,
        ] {
            loop {
                let byte = value as u8 & 127;
                value >>= 7;
                out[used] = byte | if value != 0 { 128 } else { 0 };
                used += 1;
                if value == 0 {
                    break;
                }
            }
        }
        used
    }

    pub(crate) fn read(r: &mut ByteReader<'_>) -> Result<Self, Error> {
        let kind = EntryKind::try_from(crate::trace::tf!("entry kind", r, r.varint()?))?;
        let role = u8::try_from(crate::trace::tf!("entry role", r, r.varint()?))
            .map_err(|_| Error::Corrupt("V4 directory role"))?;
        Ok(Self {
            kind,
            role,
            id: crate::trace::tf!("entry ID", r, r.varint()?),
            record_len: crate::trace::tf!("entry bytes", r, r.varint()?),
        })
    }
}

/// An entry with directory-authoritative offset and enclosing tile identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocatedEntry {
    pub entry: Entry,
    pub offset: u64,
    pub tile_id: Option<u64>,
}

impl LocatedEntry {
    /// Bind an already locally verified frame to its directory identity/length.
    /// Returns the body after its identity fields. Remaining model/unit grammar
    /// and tile membership are checked by their semantic and tile readers.
    pub fn bind<'a>(self, object: Object<'a>) -> Result<&'a [u8], Error> {
        if object.encoded_len as u64 != self.entry.record_len {
            return Err(Error::Corrupt("V4 directory/frame length mismatch"));
        }
        let mut r = ByteReader::new(object.body);
        let (kind, role, id) = match object.kind {
            crate::registry::UNIT_DC | crate::registry::UNIT_STRIPE => {
                (EntryKind::Unit, 0, r.varint()?)
            }
            crate::registry::UNIT_AC => (EntryKind::Unit, 1, r.varint()?),
            crate::registry::UNIT_ALPHA => (EntryKind::Unit, 2, r.varint()?),
            crate::registry::UNIT_EXACT => (EntryKind::Unit, 3, r.varint()?),
            crate::registry::MODEL_DEFINITION => {
                let role = r.varint()?;
                let id = r.varint()?;
                let identity_end = r.pos;
                let kind = EntryKind::model_lifetime(r.varint()?)?;
                // Preserve the return contract: model grammar starts with its
                // lifetime, immediately after the two identity fields.
                r.pos = identity_end;
                (kind, role, id)
            }
            65 => (EntryKind::TileBegin, 0, r.varint()?),
            67 => (EntryKind::TileEnd, 0, r.varint()?),
            kind if kind & 1 == 0 => (EntryKind::Ignorable, 0, self.entry.id),
            _ => return Err(Error::Unsupported("unknown V4 critical object")),
        };
        if kind != self.entry.kind || role != u64::from(self.entry.role) || id != self.entry.id {
            return Err(Error::Corrupt("V4 directory/frame identity mismatch"));
        }
        Ok(&object.body[r.pos..])
    }
}

struct Schedule {
    layout: TileLayout,
    minimum_record: u64,
    ordinal: u64,
    tile: Option<(u64, u64, u64)>, // identity, start, child count
    offset: u64,
    model_ids: [Option<u64>; 4],
    unit_ids: [Option<u64>; 4],
    ignorable: u64,
    pending_model: Option<Entry>,
    awaiting_main: bool,
    begun: bool,
}

impl Schedule {
    fn new(layout: TileLayout, integrity: ObjectIntegrity) -> Self {
        Self {
            layout,
            minimum_record: 2 + integrity.trailer_len() as u64,
            ordinal: 0,
            tile: None,
            offset: 0,
            model_ids: [None; 4],
            unit_ids: [None; 4],
            ignorable: 0,
            pending_model: None,
            awaiting_main: false,
            begun: false,
        }
    }

    fn push(
        &mut self,
        entry: Entry,
    ) -> Result<(LocatedEntry, Option<TileExtent>, Option<Entry>), Error> {
        if entry.role > 3 || entry.record_len < self.minimum_record {
            return Err(Error::Corrupt("V4 directory role/record length"));
        }
        let end = self
            .offset
            .checked_add(entry.record_len)
            .ok_or(Error::Corrupt("V4 directory extent overflow"))?;
        let mut tile_id = self.tile.map(|t| t.0);
        let mut closed = None;
        let mut model_for_unit = None;
        match entry.kind {
            EntryKind::TileBegin => {
                if self.tile.is_some()
                    || entry.role != 0
                    || entry.id != self.layout.tile_at(self.ordinal)?.id
                {
                    return Err(Error::Corrupt("V4 directory tile order/nesting"));
                }
                self.tile = Some((entry.id, self.offset, 0));
                self.unit_ids = [None; 4];
                tile_id = Some(entry.id);
            }
            EntryKind::TileEnd => {
                let (id, start, child_count) = self
                    .tile
                    .ok_or(Error::Corrupt("V4 directory end without begin"))?;
                if entry.role != 0 || entry.id != id {
                    return Err(Error::Corrupt("V4 directory tile end identity"));
                }
                closed = Some(TileExtent {
                    tile_id: id,
                    offset: start,
                    len: end - start,
                    child_count,
                });
                self.tile = None;
                self.ordinal += 1;
            }
            EntryKind::Model | EntryKind::NextModel => {
                if entry.id == 0 && (self.begun || entry.kind == EntryKind::NextModel) {
                    return Err(Error::Corrupt("V4 directory root model placement/lifetime"));
                }
                if entry.kind == EntryKind::NextModel {
                    if self.pending_model.is_some() {
                        return Err(Error::Corrupt("V4 directory two pending models"));
                    }
                    self.pending_model = Some(entry);
                }
                self.awaiting_main = true;
                let last = &mut self.model_ids[entry.role as usize];
                if last.map_or(entry.id != 0, |id| entry.id <= id) {
                    return Err(Error::Corrupt("V4 directory model identity/reuse"));
                }
                *last = Some(entry.id);
            }
            EntryKind::Unit => {
                if self
                    .pending_model
                    .is_some_and(|model| model.role != entry.role)
                {
                    return Err(Error::Corrupt("V4 directory next-model role"));
                }
                model_for_unit = self.pending_model.take();
                self.awaiting_main = false;
                self.begun = true;
                let (id, _, _) = self
                    .tile
                    .ok_or(Error::Corrupt("V4 directory unit outside tile"))?;
                if self.model_ids[entry.role as usize].is_none() && entry.role != 0 {
                    return Err(Error::Corrupt("V4 directory unit before root model"));
                }
                // Standalone numeric/Quick uses its setup root, not record 63.
                let last = &mut self.unit_ids[entry.role as usize];
                if last.is_some_and(|last| entry.id <= last) {
                    return Err(Error::Corrupt("V4 directory duplicate/out-of-order unit"));
                }
                if entry.role != 3 && self.layout.tile_for_group(entry.id)?.id != id {
                    return Err(Error::Corrupt("V4 directory unit geometry"));
                }
                if entry.role == 3 && entry.id != last.map_or(0, |last| last + 1) {
                    return Err(Error::Corrupt("V4 directory packed exact identity"));
                }
                *last = Some(entry.id);
            }
            EntryKind::Ignorable => {
                if entry.role != 0 || entry.id != self.ignorable {
                    return Err(Error::Corrupt("V4 directory ignorable ordinal"));
                }
                self.ignorable = self
                    .ignorable
                    .checked_add(1)
                    .ok_or(Error::Corrupt("V4 directory ignorable count overflow"))?;
            }
        }
        if !matches!(entry.kind, EntryKind::TileBegin | EntryKind::TileEnd)
            && let Some(tile) = &mut self.tile
        {
            tile.2 = tile
                .2
                .checked_add(1)
                .ok_or(Error::Corrupt("V4 directory child count overflow"))?;
        }
        let located = LocatedEntry {
            entry,
            offset: self.offset,
            tile_id,
        };
        self.offset = end;
        Ok((located, closed, model_for_unit))
    }

    fn finish(&self, objects_len: u64) -> Result<(), Error> {
        if self.awaiting_main
            || self.pending_model.is_some()
            || self.tile.is_some()
            || self.ordinal != self.layout.tile_count()
            || self.offset != objects_len
        {
            return Err(Error::Corrupt("V4 directory coverage/extent"));
        }
        Ok(())
    }
}

/// Directory writer state. Append each returned fixed buffer to caller-owned
/// scratch; at finalization replay count, scratch entries and objects_len.
///
/// It retains only scalar schedule state. No output is published by this type.
/// On error discard the directory builder; its schedule is poisoned.
pub struct DirectoryWriter {
    schedule: Schedule,
    count: u64,
    failed: bool,
}

impl DirectoryWriter {
    pub fn new(layout: TileLayout, integrity: ObjectIntegrity) -> Self {
        Self {
            schedule: Schedule::new(layout, integrity),
            count: 0,
            failed: false,
        }
    }

    /// Validate/encode one entry into caller-owned fixed space. A complete
    /// pixel/model schedule is additionally checked by the semantic codec.
    pub fn push(&mut self, entry: Entry, out: &mut [u8; 40]) -> Result<usize, Error> {
        if self.failed {
            return Err(Error::InvalidInput("failed V4 directory writer"));
        }
        let count = self.count.checked_add(1).ok_or(Error::TooLarge)?;
        let result = self.schedule.push(entry);
        if result.is_err() {
            self.failed = true;
        }
        result?;
        self.count = count;
        Ok(entry.encode(out))
    }

    /// Count prefix and objects_len suffix for the identical front/tail body.
    pub fn finish(&self) -> Result<(u64, u64), Error> {
        if self.failed {
            return Err(Error::InvalidInput("failed V4 directory writer"));
        }
        self.schedule.finish(self.schedule.offset)?;
        Ok((self.count, self.schedule.offset))
    }
}

/// Borrowed directory validated for its declared layout, identities, complete object-stream
/// extent and tile boundaries. No allocation is proportional to an ID/count.
///
/// This validates directory structure, not unread physical prefixes, definition
/// lifetimes, complete semantic unit coverage or entropy. Those remain codec
/// checks. Lookups scan the compact directory and charge that work; a host may
/// cache a separately admitted lookup accelerator. Whole-file structural
/// validation must reconcile the observed record stream with these entries.
pub struct Directory<'a> {
    entries: &'a [u8],
    count: u64,
    objects_len: u64,
    layout: TileLayout,
    mode: ObjectIntegrity,
    ends_at_tile: bool,
}

impl<'a> Directory<'a> {
    pub fn read<C: Fn() -> bool + Sync>(
        bytes: &'a [u8],
        layout: TileLayout,
        mode: ObjectIntegrity,
        expected_objects_len: u64,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        session.v4_input(bytes.len())?;
        Self::read_admitted(bytes, layout, mode, expected_objects_len, session)
    }

    /// Front reservations already charged their complete serialized input.
    /// Still charge every parse/validation step and preserve all shape checks.
    pub(crate) fn read_admitted<C: Fn() -> bool + Sync>(
        bytes: &'a [u8],
        layout: TileLayout,
        mode: ObjectIntegrity,
        expected_objects_len: u64,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        session.adapter_work(1)?;
        let mut r = ByteReader::new(bytes);
        crate::trace::trace_enter_addr("directory", "V4:6", "unit/model/tile directory", r.mark());
        let count = crate::trace::tf!("directory entries", r, r.varint()?);
        let entry_start = r.pos;
        if r.remaining() < 8 || count > ((r.remaining() - 8) / 4) as u64 {
            return Err(Error::Corrupt("V4 directory count"));
        }
        let mut schedule = Schedule::new(layout, mode);
        let mut ends_at_tile = false;
        for _ in 0..count {
            let start = r.pos;
            let entry = Entry::read(&mut r)?;
            ends_at_tile = entry.kind == EntryKind::TileEnd;
            session.adapter_work((r.pos - start) as u128 + 1)?;
            schedule.push(entry)?;
        }
        let entry_end = r.pos;
        let objects_len = crate::trace::tf!(
            "object stream bytes",
            r,
            u64::from_le_bytes(r.bytes(8)?.try_into().unwrap())
        );
        if r.remaining() != 0 || objects_len != expected_objects_len {
            return Err(Error::Corrupt("V4 directory data length/trailing bytes"));
        }
        schedule.finish(objects_len)?;
        crate::trace::exit_addr(r.mark());
        Ok(Self {
            entries: &bytes[entry_start..entry_end],
            count,
            objects_len,
            ends_at_tile,
            layout,
            mode,
        })
    }

    pub const fn entry_count(&self) -> u64 {
        self.count
    }
    pub const fn objects_len(&self) -> u64 {
        self.objects_len
    }

    pub(crate) const fn ends_at_tile(&self) -> bool {
        self.ends_at_tile
    }

    #[cfg(feature = "std")]
    pub(crate) fn entries_reader(&self) -> ByteReader<'a> {
        ByteReader::new(self.entries)
    }

    /// Walk checked entries. The callback may stop early by returning false.
    /// Every visit remains charged, including repeated lookups.
    pub fn visit<C: Fn() -> bool + Sync>(
        &self,
        session: &DecodeSession<C>,
        mut visit: impl FnMut(LocatedEntry, Option<TileExtent>) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.visit_positions(session, |entry, tile, _| visit(entry, tile))
    }

    pub(super) fn visit_positions<C: Fn() -> bool + Sync>(
        &self,
        session: &DecodeSession<C>,
        mut visit: impl FnMut(
            LocatedEntry,
            Option<TileExtent>,
            core::ops::Range<usize>,
        ) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.visit_bindings(session, |entry, tile, bytes, _| visit(entry, tile, bytes))
    }

    /// The consumed next-unit definition comes entirely from authoritative
    /// directory metadata. A sparse loader still binds fetched definition and
    /// unit fields to it; no preceding entropy or unit prefix is needed.
    pub(crate) fn visit_bindings<C: Fn() -> bool + Sync>(
        &self,
        session: &DecodeSession<C>,
        mut visit: impl FnMut(
            LocatedEntry,
            Option<TileExtent>,
            core::ops::Range<usize>,
            Option<Entry>,
        ) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        let mut r = ByteReader::new(self.entries);
        let mut schedule = Schedule::new(self.layout, self.mode);
        for _ in 0..self.count {
            let start = r.pos;
            let entry = Entry::read(&mut r)?;
            session.adapter_work((r.pos - start) as u128 + 1)?;
            let (entry, tile, model) = schedule.push(entry)?;
            let base = crate::varint_len(self.count);
            if !visit(entry, tile, base + start..base + r.pos, model)? {
                break;
            }
        }
        Ok(())
    }

    /// Return the complete contiguous tile range. Shared definitions can be
    /// separate dependencies; no other tile's pixel entropy is required.
    pub fn tile<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        session: &DecodeSession<C>,
    ) -> Result<TileExtent, Error> {
        self.layout.tile(id)?;
        let mut result = None;
        self.visit(session, |_, tile| {
            if let Some(tile) = tile
                && tile.tile_id == id
            {
                result = Some(tile);
                return Ok(false);
            }
            Ok(true)
        })?;
        result.ok_or(Error::Corrupt("missing V4 directory tile"))
    }

    /// Resolve a unit in its tile, or a payload-wide model definition. Other
    /// entry kinds use tile()/visit(). Model selects either model lifetime;
    /// NextModel selects only next-unit definitions. The returned entry retains
    /// the actual lifetime. This does not validate fetched bytes.
    pub fn find<C: Fn() -> bool + Sync>(
        &self,
        kind: EntryKind,
        role: u8,
        id: u64,
        tile_id: Option<u64>,
        session: &DecodeSession<C>,
    ) -> Result<LocatedEntry, Error> {
        if role > 3
            || !matches!(
                kind,
                EntryKind::Unit | EntryKind::Model | EntryKind::NextModel
            )
            || (kind == EntryKind::Unit && tile_id.is_none())
            || (kind.is_model() && tile_id.is_some())
        {
            return Err(Error::InvalidInput("V4 directory lookup key"));
        }
        let mut result = None;
        self.visit(session, |entry, _| {
            if (entry.entry.kind == kind
                || (kind == EntryKind::Model && entry.entry.kind == EntryKind::NextModel))
                && entry.entry.role == role
                && entry.entry.id == id
                && (kind.is_model() || entry.tile_id == tile_id)
            {
                result = Some(entry);
                return Ok(false);
            }
            Ok(true)
        })?;
        result.ok_or(Error::Corrupt("missing V4 directory identity"))
    }
}

#[cfg(test)]
mod tests;
