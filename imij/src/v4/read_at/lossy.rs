//! Indexed lossy definitions and fetched-tile planning share the forward grammar.
use super::*;
use crate::lossy::v4::{Definition, Model, ModelResolver, StoredLossyUnit};

pub(super) struct ModelSpan {
    pub(super) entry: LocatedEntry,
    target: Option<u64>,
    target_offset: Option<u64>,
    pub(super) cached: Option<Model>,
}

pub(super) fn index<C: Fn() -> bool + Sync>(
    directory: &Directory<'_>,
    root: &Root,
    tiles: &mut [Option<TileSpan>],
    session: &DecodeSession<C>,
) -> Result<(Vec<ModelSpan>, Vec<(u64, u8, u64)>, V4Retained), Error> {
    let mut count = 0usize;
    directory.visit(session, |entry, _| {
        if entry.entry.kind.is_model() {
            count = count.checked_add(1).ok_or(Error::TooLarge)?;
        }
        Ok(true)
    })?;
    let memory = session.v4_working(
        count as u128 * (size_of::<ModelSpan>() + size_of::<(u64, u8, u64)>()) as u128,
    )?;
    let mut models: Vec<ModelSpan> = Vec::new();
    models
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    let mut next_models = Vec::new();
    next_models
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    let mut roots = [false; 4];
    let mut pending = None;
    let mut begin = None;
    let mut ignorable = 0;
    let mut units = [0u64; 4];
    directory.visit_bindings(session, |entry, end, bytes, binding| {
        let e = entry.entry;
        let numeric_role = |role: u8| match root {
            Root::Numeric(_) => role == 0,
            Root::Lossy { setup, .. } => {
                role == 1 || setup.numeric_geometry(u64::from(role)).is_some()
            }
        };
        if e.kind.is_model() {
            if matches!(root, Root::Numeric(_)) {
                return Err(Error::Unsupported("V4 standalone numeric model definition"));
            }
            if !numeric_role(e.role) {
                return Err(Error::Corrupt("V4 indexed model absent plane"));
            }
            if e.id == 0 {
                roots[e.role as usize] = true;
            }
            if e.kind == EntryKind::NextModel {
                pending = Some(models.len());
            }
            models.push(ModelSpan {
                entry,
                target: None,
                target_offset: None,
                cached: None,
            });
        }
        if e.kind == EntryKind::Unit {
            if !numeric_role(e.role) {
                return Err(Error::Corrupt("V4 indexed main absent plane"));
            }
            if matches!(root, Root::Lossy { .. })
                && (0..4).any(|r| numeric_role(r) && !roots[r as usize])
            {
                return Err(Error::Corrupt("V4 indexed roots missing before main"));
            }
            if let Some(model) = binding {
                let i = pending
                    .take()
                    .ok_or(Error::Corrupt("V4 indexed pending binding"))?;
                if models[i].entry.entry != model {
                    return Err(Error::Corrupt("V4 indexed pending identity"));
                }
                models[i].target = Some(e.id);
                models[i].target_offset = Some(entry.offset);
                next_models.push((entry.offset, model.role, model.id));
            }
            if e.role == 0
                && let Root::Lossy { setup, .. } = root
                && setup.dc_for_group(e.id)?.0 != e.id
            {
                return Err(Error::Corrupt("V4 indexed DC span identity"));
            }
            units[e.role as usize] = units[e.role as usize]
                .checked_add(1)
                .ok_or(Error::TooLarge)?;
        }
        if e.kind == EntryKind::TileBegin {
            begin = Some((bytes.start, ignorable));
            units = [0; 4];
        }
        if e.kind == EntryKind::Ignorable {
            ignorable = e.id.checked_add(1).ok_or(Error::TooLarge)?;
        }
        if let Some(extent) = end {
            let tile = root.layout().tile(extent.tile_id)?;
            let expected = match root {
                Root::Numeric(_) => [tile.group_count(), 0, 0],
                Root::Lossy { setup, .. } => {
                    let mut dc = 0;
                    for group in setup.layout().groups(tile.id)? {
                        session.adapter_work(1)?;
                        if setup.dc_for_group(group.id)?.0 == group.id {
                            dc += 1;
                        }
                    }
                    [
                        dc,
                        tile.group_count(),
                        if setup.numeric_geometry(2).is_some() {
                            tile.group_count()
                        } else {
                            0
                        },
                    ]
                }
            };
            if units[..3] != expected {
                return Err(Error::Corrupt("V4 indexed main group coverage"));
            }
            let (entry_start, first_ignorable) = begin
                .take()
                .ok_or(Error::Corrupt("V4 tile accelerator nesting"))?;
            tiles[extent.tile_id as usize] = Some(TileSpan {
                extent,
                entry_start,
                entry_end: bytes.end,
                first_ignorable,
            });
        }
        Ok(true)
    })?;
    session.adapter_work(models.len() as u128 * 64)?;
    models.sort_unstable_by_key(|m| (m.entry.entry.role, m.entry.entry.id));
    Ok((models, next_models, memory.retain()?))
}

impl<R: ReadAt> IndexedFile<R> {
    fn model_index(&self, role: u8, id: u64) -> Result<usize, Error> {
        self.models
            .binary_search_by_key(&(role, id), |m| (m.entry.entry.role, m.entry.entry.id))
            .map_err(|_| Error::Corrupt("undefined or expired V4 indexed model"))
    }
    pub(super) fn load_roots<C: Fn() -> bool + Sync>(
        &mut self,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if matches!(self.root, Root::Numeric(_)) {
            return Ok(());
        }
        for role in 0..4 {
            if let Ok(i) = self.model_index(role, 0) {
                self.load_model(i, None, session)?;
            }
        }
        Ok(())
    }
    fn validate_definition<'a>(
        &self,
        body: &'a [u8],
        expected: LocatedEntry,
    ) -> Result<Definition<'a>, Error> {
        let Root::Lossy { setup, .. } = &self.root else {
            return Err(Error::Unsupported("V4 indexed lossy model"));
        };
        let definition = Definition::read(setup, body)?;
        let span = &self.models[self.model_index(definition.role, definition.id)?];
        if span.entry.offset != expected.offset
            || span.entry.entry != expected.entry
            || definition.target != span.target
        {
            return Err(Error::Corrupt(
                "V4 indexed definition binding/lifetime/target",
            ));
        }
        Ok(definition)
    }
    fn load_model<C: Fn() -> bool + Sync>(
        &mut self,
        index: usize,
        tile: Option<(u64, &[u8])>,
        session: &DecodeSession<C>,
    ) -> Result<Model, Error> {
        if let Some(model) = &self.models[index].cached {
            session.adapter_work(1)?;
            return Ok(model.clone());
        }
        let entry = self.models[index].entry;
        if entry.entry.record_len > self.limits.max_object_bytes {
            return Err(Error::TooLarge);
        }
        let at = self
            .objects_at
            .checked_add(entry.offset)
            .ok_or(Error::TooLarge)?;
        let count = usize::try_from(entry.entry.record_len).map_err(|_| Error::TooLarge)?;
        let end = at.checked_add(count as u64).ok_or(Error::TooLarge)?;
        let contained = tile.and_then(|(start, bytes)| {
            let a = usize::try_from(at.checked_sub(start)?).ok()?;
            bytes.get(a..a.checked_add(count)?)
        });
        let mut owned = Vec::new();
        let memory;
        let bytes = if let Some(bytes) = contained {
            bytes
        } else {
            memory = session.v4_working(count as u128)?;
            session.v4_check_input(count)?;
            owned
                .try_reserve_exact(count)
                .map_err(|_| Error::TooLarge)?;
            owned.resize(count, 0);
            session.v4_input(count)?;
            session.adapter_work(count as u128 + 1)?;
            self.source.read_exact_at(at, &mut owned)?;
            if tile.is_some() {
                self.verify_used((at, end), at, &owned, session)?;
            } else if let Some(check) = &mut self.check {
                check.opened.push(at, end, session)?;
            }
            let _ = &memory;
            &owned
        };
        let Root::Lossy { setup, context } = &self.root else {
            return Err(Error::Unsupported("V4 indexed lossy model"));
        };
        let object = super::super::ObjectReader::new(
            setup.integrity(),
            *context,
            session,
            self.limits.max_object_bytes,
        )
        .read_admitted(bytes)?;
        entry.bind(object)?;
        let definition = self.validate_definition(object.body, entry)?;
        let model = definition.expand(setup, session)?;
        if entry.entry.kind == EntryKind::Model {
            if entry.entry.id != 0 {
                if self.cached_models.len() == self.cached_models.capacity() {
                    let count = self
                        .cached_models
                        .capacity()
                        .max(4)
                        .checked_mul(2)
                        .ok_or(Error::TooLarge)?;
                    let memory = session.v4_working(count as u128 * size_of::<usize>() as u128)?;
                    self.cached_models
                        .try_reserve_exact(count - self.cached_models.len())
                        .map_err(|_| Error::TooLarge)?;
                    self.cache_memory = memory.retain()?;
                }
                self.cached_models.push(index);
            }
            self.models[index].cached = Some(model.clone());
        }
        Ok(model)
    }

    pub(crate) fn visit_lossy_tile<'a, C: Fn() -> bool + Sync>(
        &mut self,
        id: u64,
        bytes: &'a [u8],
        session: &DecodeSession<C>,
        mut visit: impl FnMut(super::super::LossyEvent<'a>) -> Result<(), Error>,
    ) -> Result<TileExtent, Error> {
        let span = self.span(id, session)?;
        if bytes.len() as u64 != span.extent.len {
            return Err(Error::Corrupt("V4 fetched tile length"));
        }
        let Root::Lossy { setup, context } = &self.root else {
            return Err(Error::Unsupported("V4 indexed lossy tile"));
        };
        let mut decoder = super::super::LossyGroupDecoder::at_indexed_tile(
            Arc::clone(setup),
            *context,
            self.limits,
            session,
            id,
            span.extent.offset,
            span.first_ignorable,
        )?;
        // Copy only this tile's compact index. It permits the resolver to fetch
        // missing definitions while the shared semantic driver advances.
        let index_len = span.entry_end - span.entry_start;
        let _index_memory = session.v4_working(index_len as u128)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(index_len)
            .map_err(|_| Error::TooLarge)?;
        entries.extend_from_slice(&self.directory[span.entry_start..span.entry_end]);
        let mut r = ByteReader::new(&entries);
        let mut offset = 0usize;
        let tile_at = self
            .objects_at
            .checked_add(span.extent.offset)
            .ok_or(Error::TooLarge)?;
        let mut closed = None;
        while r.remaining() != 0 {
            let entry = Entry::read(&mut r)?;
            session.adapter_work(1)?;
            let end = offset
                .checked_add(usize::try_from(entry.record_len).map_err(|_| Error::TooLarge)?)
                .ok_or(Error::TooLarge)?;
            let input = bytes
                .get(offset..end)
                .ok_or(Error::Corrupt("V4 fetched/indexed tile extent"))?;
            let expected = LocatedEntry {
                entry,
                offset: span.extent.offset + offset as u64,
                tile_id: Some(id),
            };
            let mut resolver = Resolver {
                file: self,
                tile: (tile_at, bytes),
                current: expected,
                session,
            };
            let event = decoder.read_resolved(input, &mut resolver)?;
            if event.entry() != expected {
                return Err(Error::Corrupt("V4 fetched/directory identity"));
            }
            if let super::super::LossyEvent::End { tile, .. } = &event {
                closed = Some(*tile);
            }
            visit(event)?;
            offset = end;
        }
        if offset != bytes.len() || closed != Some(span.extent) {
            return Err(Error::Corrupt("V4 fetched tile completion"));
        }
        Ok(span.extent)
    }
}

struct Resolver<'a, 's, R, C> {
    file: &'a mut IndexedFile<R>,
    tile: (u64, &'a [u8]),
    current: LocatedEntry,
    session: &'s DecodeSession<C>,
}
impl<R: ReadAt, C: Fn() -> bool + Sync> ModelResolver for Resolver<'_, '_, R, C> {
    #[cfg(all(feature = "lab", feature = "std"))]
    fn trace_settings(
        &self,
        setup: &crate::v4::LossySetup,
        id: u64,
        settings: &[u8],
    ) -> Result<(), Error> {
        setup.trace_settings(id, settings, self.session)
    }

    fn define(&mut self, body: &[u8]) -> Result<(), Error> {
        self.session.adapter_work(body.len() as u128 + 1)?;
        self.file
            .validate_definition(body, self.current)
            .map(|_| ())
    }
    fn take(&mut self, role: u8, unit: u64, id: u64) -> Result<Model, Error> {
        self.session.adapter_work(1)?;
        let i = self.file.model_index(role, id)?;
        let model = &self.file.models[i];
        if model.entry.offset >= self.current.offset
            || model.target.is_some_and(|target| target != unit)
            || model.entry.entry.kind == EntryKind::NextModel
                && (model.target != Some(unit) || model.target_offset != Some(self.current.offset))
        {
            return Err(Error::Corrupt("V4 indexed model forward reference/target"));
        }
        // A selected persistent model must not bypass a pending next-main one.
        if let Ok(i) = self
            .file
            .next_models
            .binary_search_by_key(&self.current.offset, |v| v.0)
        {
            let (_, expected_role, expected_id) = self.file.next_models[i];
            if (expected_role, expected_id) != (role, id) {
                return Err(Error::Corrupt("V4 indexed pending model selection"));
            }
        }
        self.file.load_model(i, Some(self.tile), self.session)
    }
}

pub(crate) struct TilePlan {
    units: Vec<(u8, u64, StoredLossyUnit)>,
    exact: Option<Arc<super::super::ExactPlan>>,
    tile: super::super::SpatialTile,
    columns: u64,
    _memory: V4Retained,
}
impl TilePlan {
    fn unit<'a>(
        &self,
        role: u8,
        id: u64,
        bytes: &'a [u8],
    ) -> Result<super::super::LossyUnit<'a>, Error> {
        let i = self
            .units
            .binary_search_by_key(&(role, id), |u| (u.0, u.1))
            .map_err(|_| Error::Corrupt("V4 fetched dependency missing"))?;
        self.units[i].2.bind(bytes)
    }
    pub(crate) fn group<'a, C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        bytes: &'a [u8],
        session: &DecodeSession<C>,
        dc: &mut crate::lossy::v4::NumericDependency,
        exact: &mut crate::lossy::v4::NumericDependency,
    ) -> Result<super::super::LossyWork<'a>, Error> {
        let unit = self.unit(1, id, bytes)?;
        let setup = unit.setup();
        let source = self.unit(0, setup.dc_for_group(id)?.0, bytes)?;
        let alpha = setup
            .numeric_geometry(2)
            .map(|_| self.unit(2, id, bytes))
            .transpose()?;
        let ordinal = (id / self.columns - u64::from(self.tile.group_y))
            * u64::from(self.tile.groups_wide)
            + id % self.columns
            - u64::from(self.tile.group_x);
        crate::lossy::v4::prepare_group(
            unit,
            source,
            alpha,
            self.exact.as_deref().map(|p| (p, ordinal)),
            |stripe| self.unit(3, stripe, bytes),
            session,
            dc,
            exact,
        )
    }
}
impl<R: ReadAt> IndexedFile<R> {
    pub(crate) fn plan_lossy_tile<C: Fn() -> bool + Sync>(
        &mut self,
        id: u64,
        bytes: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<TilePlan, Error> {
        let span = self.span(id, session)?;
        let mut entries = ByteReader::new(&self.directory[span.entry_start..span.entry_end]);
        let mut count = 0usize;
        while entries.remaining() != 0 {
            session.adapter_work(1)?;
            if Entry::read(&mut entries)?.kind == EntryKind::Unit {
                count = count.checked_add(1).ok_or(Error::TooLarge)?;
            }
        }
        let memory =
            session.v4_working(count as u128 * size_of::<(u8, u64, StoredLossyUnit)>() as u128)?;
        let mut units = Vec::new();
        units
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        let mut exact = None;
        self.visit_lossy_tile(id, bytes, session, |event| {
            match event {
                super::super::LossyEvent::Begin { exact: plan, .. } => exact = plan,
                super::super::LossyEvent::Unit { work, .. } => {
                    units.push((work.role(), work.id(), work.store(bytes)?))
                }
                _ => {}
            }
            Ok(())
        })?;
        session.adapter_work(units.len() as u128 * 64)?;
        units.sort_unstable_by_key(|u| (u.0, u.1));
        Ok(TilePlan {
            units,
            exact,
            tile: self.root.layout().tile(id)?,
            columns: u64::from(self.root.layout().group_grid().0),
            _memory: memory.retain()?,
        })
    }
}
