//! V4 lossy object schedule. Pixel reconstruction remains independent work.

use alloc::{sync::Arc, vec::Vec};
use core::ops::Range;

use super::{LossyModels, LossySetup, LossyUnit};
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::planes::Rect;
use crate::v4::{
    DirectoryPrefix, Entry, EntryKind, Group, Groups, LocatedEntry, ObjectExtent, SetupContext,
    SpatialTile, TileEvent, TileExtent, TileLimits, TileReader,
};
use crate::{ByteReader, Error, Features, ImageInfo, LossyColor};

/// Tile-local exact packing. Only block counts are serialized in begin;
/// stripe count/ownership derive from geometry and every stripe has a frame.
pub struct ExactPlan {
    ends: Vec<u64>,
    packed_blocks: u32,
    rows: u32,
    height: u32,
    _charge: crate::decode_resources::V4Retained,
}

impl ExactPlan {
    fn read<C: Fn() -> bool + Sync>(
        setup: &LossySetup,
        tile: SpatialTile,
        bytes: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        let (packed_blocks, rows) = setup
            .exact
            .ok_or(Error::Corrupt("unexpected V4 exact plan"))?;
        let mut outer = ByteReader::new(bytes);
        let length = outer.varint()?;
        let body = outer.bytes_len(length)?;
        if outer.remaining() != 0 {
            return Err(Error::Corrupt("V4 exact plan trailing bytes"));
        }
        if tile.group_count() > body.len() as u64 {
            return Err(Error::Corrupt("V4 exact plan count bounds"));
        }
        let count = usize::try_from(tile.group_count()).map_err(|_| Error::TooLarge)?;
        let capacity = count
            .checked_add(1)
            .and_then(|n| n.checked_mul(8))
            .ok_or(Error::TooLarge)?;
        session.v4_models().hint(
            capacity
                .checked_add(core::mem::size_of::<Self>())
                .ok_or(Error::TooLarge)?,
        )?;
        session.adapter_work(body.len() as u128 + u128::from(tile.group_count()) + 1)?;
        let mut ends = Vec::new();
        ends.try_reserve_exact(count + 1)
            .map_err(|_| Error::TooLarge)?;
        ends.push(0u64);
        let mut r = ByteReader::new(body);
        for group in setup.layout.groups(tile.id)? {
            session.adapter_work(1)?;
            let rect = setup.group_rect(group.id)?;
            let blocks = r.varint()?;
            if blocks > u64::from(rect.width().div_ceil(8)) * u64::from(rect.height().div_ceil(8)) {
                return Err(Error::Corrupt("V4 exact group block count"));
            }
            ends.push(
                ends.last()
                    .unwrap()
                    .checked_add(blocks)
                    .ok_or(Error::TooLarge)?,
            );
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("V4 exact plan trailing fields"));
        }
        let height = u32::try_from(
            ends.last()
                .unwrap()
                .div_ceil(u64::from(packed_blocks))
                .checked_mul(8)
                .ok_or(Error::TooLarge)?,
        )
        .map_err(|_| Error::TooLarge)?;
        Ok(Self {
            ends,
            packed_blocks,
            rows,
            height,
            _charge: scope.commit()?,
        })
    }

    pub fn group_blocks(&self, group: u64) -> Result<Range<u64>, Error> {
        let i = usize::try_from(group).map_err(|_| Error::TooLarge)?;
        let end = self
            .ends
            .get(i.checked_add(1).ok_or(Error::TooLarge)?)
            .ok_or(Error::Corrupt("V4 exact group ordinal"))?;
        Ok(self.ends[i]..*end)
    }

    pub fn stripe_count(&self) -> u64 {
        u64::from(self.height.div_ceil(self.rows))
    }
    pub fn blocks_per_stripe(&self) -> u64 {
        u64::from(self.packed_blocks) * u64::from(self.rows / 8)
    }

    pub fn stripe_rect(&self, stripe: u64) -> Result<Rect, Error> {
        if stripe >= self.stripe_count() {
            return Err(Error::Corrupt("V4 exact stripe ordinal"));
        }
        let y = stripe * u64::from(self.rows);
        Ok(Rect::new(
            0,
            y as u32,
            self.packed_blocks * 8,
            (u64::from(self.height) - y).min(u64::from(self.rows)) as u32,
        ))
    }

    pub fn carried_by(&self, group: u64) -> Result<Range<u64>, Error> {
        let blocks = self.group_blocks(group)?;
        let per = self.blocks_per_stripe();
        Ok(blocks.start.div_ceil(per)..blocks.end.div_ceil(per))
    }
}

/// A structurally checked object. Units own immutable models while borrowing
/// caller-held compressed bytes. Completion scopes remain explicit.
pub enum LossyEvent<'a> {
    Begin {
        tile: SpatialTile,
        exact: Option<Arc<ExactPlan>>,
        entry: LocatedEntry,
    },
    Definition {
        entry: LocatedEntry,
    },
    Unit {
        work: LossyUnit<'a>,
        entry: LocatedEntry,
    },
    Ancillary {
        kind: u64,
        body: &'a [u8],
        entry: LocatedEntry,
    },
    End {
        pixel_results: &'a [u8],
        tile: TileExtent,
        entry: LocatedEntry,
    },
}

impl LossyEvent<'_> {
    pub fn entry(&self) -> LocatedEntry {
        match self {
            Self::Begin { entry, .. }
            | Self::Definition { entry }
            | Self::Unit { entry, .. }
            | Self::Ancillary { entry, .. }
            | Self::End { entry, .. } => *entry,
        }
    }
}

fn located(extent: ObjectExtent, kind: EntryKind, role: u8, id: u64) -> LocatedEntry {
    LocatedEntry {
        entry: Entry {
            kind,
            role,
            id,
            record_len: extent.len,
        },
        offset: extent.offset,
        tile_id: extent.tile_id,
    }
}

/// Forward semantic driver over complete framed objects. It retains models
/// and one tile's packing metadata, never decoded pixels or an image index.
pub struct LossyGroupDecoder<'s, C> {
    setup: Arc<LossySetup>,
    models: LossyModels<'s, C>,
    frames: TileReader<'s, C>,
    groups: Option<Groups>,
    current: Option<Group>,
    group_ordinal: u64,
    phase: u8,
    exact: Option<Arc<ExactPlan>>,
    exact_next: Option<Range<u64>>,
    tile: Option<u64>,
    ignorable: u64,
    failed: bool,
    session: &'s DecodeSession<C>,
}

#[cfg(feature = "std")]
pub(crate) struct LossyState {
    setup: Arc<LossySetup>,
    models: super::models::ModelsState,
    frames: crate::v4::TileState,
    groups: Option<Groups>,
    current: Option<Group>,
    group_ordinal: u64,
    phase: u8,
    exact: Option<Arc<ExactPlan>>,
    exact_next: Option<Range<u64>>,
    tile: Option<u64>,
    ignorable: u64,
    failed: bool,
}
#[cfg(feature = "std")]
impl LossyState {
    pub(crate) fn group_rect(&self, id: u64) -> Result<Rect, Error> {
        self.setup.group_rect(id)
    }

    pub(crate) fn bind<C: Fn() -> bool + Sync>(
        self,
        session: &DecodeSession<C>,
    ) -> LossyGroupDecoder<'_, C> {
        LossyGroupDecoder {
            setup: self.setup,
            models: self.models.bind(session),
            frames: self.frames.bind(session),
            groups: self.groups,
            current: self.current,
            group_ordinal: self.group_ordinal,
            phase: self.phase,
            exact: self.exact,
            exact_next: self.exact_next,
            tile: self.tile,
            ignorable: self.ignorable,
            failed: self.failed,
            session,
        }
    }
}

impl<'s, C: Fn() -> bool + Sync> LossyGroupDecoder<'s, C> {
    #[cfg(feature = "std")]
    pub(crate) fn into_state(self) -> LossyState {
        LossyState {
            setup: self.setup,
            models: self.models.into_state(),
            frames: self.frames.into_state(),
            groups: self.groups,
            current: self.current,
            group_ordinal: self.group_ordinal,
            phase: self.phase,
            exact: self.exact,
            exact_next: self.exact_next,
            tile: self.tile,
            ignorable: self.ignorable,
            failed: self.failed,
        }
    }
    pub fn new(
        info: ImageInfo,
        color: LossyColor,
        declared: Features,
        input: &[u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<(Self, usize), Error> {
        Self::with_directory(info, color, declared, input, limits, session)
            .map(|(decoder, n, _)| (decoder, n))
    }

    pub(crate) fn with_directory<'a>(
        info: ImageInfo,
        color: LossyColor,
        declared: Features,
        input: &'a [u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<(Self, usize, DirectoryPrefix<'a>), Error> {
        Self::with_inherited_directory(info, color, declared, input, limits, session, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_inherited_directory<'a>(
        stored_info: ImageInfo,
        color: LossyColor,
        declared: Features,
        input: &'a [u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
        inherited: Option<&ImageInfo>,
    ) -> Result<(Self, usize, DirectoryPrefix<'a>), Error> {
        let mut info = stored_info.clone();
        if let Some(parent) = inherited {
            info.cicp = parent.cicp;
            info.alpha_mode = parent.alpha_mode;
        }
        let mut r = ByteReader::new(input);
        crate::trace::trace_enter_addr("payload", "V4:2.2", "lossy setup", r.mark());
        let length = crate::trace::tf!("setup bytes", r, r.varint()?);
        let body = r.bytes_len(length)?;
        session.v4_setup(r.pos)?;
        let setup = Arc::new(LossySetup::read(info, color, declared, body, session)?);
        let context = SetupContext::from_image_declaration(
            &stored_info,
            crate::Coding::Lossy,
            Some(color),
            body,
        )?;
        crate::trace::exit_addr(r.mark());
        let storage = DirectoryPrefix::read(&input[r.pos..], session)?;
        let used = r
            .pos
            .checked_add(storage.encoded_len)
            .ok_or(Error::TooLarge)?;
        let frames = TileReader::new(setup.layout, setup.integrity, context, limits, session)
            .with_checks(setup.checks.clone());
        let models = LossyModels::new(Arc::clone(&setup), session);
        Ok((
            Self {
                setup,
                models,
                frames,
                groups: None,
                current: None,
                group_ordinal: 0,
                phase: 0,
                exact: None,
                exact_next: None,
                tile: None,
                ignorable: 0,
                failed: false,
                session,
            },
            used,
            storage,
        ))
    }

    pub(crate) fn at_indexed_tile(
        setup: Arc<LossySetup>,
        context: SetupContext,
        limits: TileLimits,
        session: &'s DecodeSession<C>,
        id: u64,
        offset: u64,
        ignorable: u64,
    ) -> Result<Self, Error> {
        let frames = TileReader::new(setup.layout, setup.integrity, context, limits, session)
            .with_checks(setup.checks.clone())
            .at_tile(setup.layout.tile_ordinal(id)?, offset)?
            .admitted();
        Ok(Self {
            models: LossyModels::new(Arc::clone(&setup), session),
            setup,
            frames,
            groups: None,
            current: None,
            group_ordinal: 0,
            phase: 0,
            exact: None,
            exact_next: None,
            tile: None,
            ignorable,
            failed: false,
            session,
        })
    }

    pub fn setup(&self) -> &LossySetup {
        &self.setup
    }

    pub(crate) fn shared_setup(&self) -> Arc<LossySetup> {
        Arc::clone(&self.setup)
    }

    fn expected(&mut self) -> Result<Option<(u8, u64)>, Error> {
        loop {
            let Some(group) = self.current else {
                return Ok(None);
            };
            match self.phase {
                0 => {
                    if self.setup.dc_for_group(group.id)?.0 == group.id {
                        return Ok(Some((0, group.id)));
                    }
                    self.phase = 1;
                }
                1 => {
                    if self.setup.numeric[2].is_some() {
                        return Ok(Some((2, group.id)));
                    }
                    self.phase = 2;
                }
                2 => return Ok(Some((1, group.id))),
                _ => {
                    if let Some(plan) = &self.exact {
                        if self.exact_next.is_none() {
                            self.exact_next = Some(plan.carried_by(self.group_ordinal)?);
                        }
                        let range = self.exact_next.as_ref().unwrap();
                        if range.start < range.end {
                            return Ok(Some((3, range.start)));
                        }
                    }
                    self.current = self.groups.as_mut().and_then(Iterator::next);
                    self.group_ordinal += 1;
                    self.phase = 0;
                    self.exact_next = None;
                }
            }
        }
    }

    pub fn read<'a>(&mut self, input: &'a [u8]) -> Result<LossyEvent<'a>, Error> {
        if self.failed {
            return Err(Error::Corrupt("failed V4 lossy object stream"));
        }
        let result = self.read_inner(input, None);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    pub(crate) fn read_resolved<'a>(
        &mut self,
        input: &'a [u8],
        resolver: &mut dyn super::ModelResolver,
    ) -> Result<LossyEvent<'a>, Error> {
        if self.failed {
            return Err(Error::Corrupt("failed V4 lossy object stream"));
        }
        let result = self.read_inner(input, Some(resolver));
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_inner<'a>(
        &mut self,
        input: &'a [u8],
        resolver: Option<&mut dyn super::ModelResolver>,
    ) -> Result<LossyEvent<'a>, Error> {
        match self.frames.read(input)? {
            TileEvent::Begin { extent, exact_plan } => {
                let id = extent.tile_id.unwrap();
                let tile = self.setup.layout.tile(id)?;
                if crate::trace::on() {
                    let columns = u64::from(self.setup.layout.group_grid().0);
                    let first = self
                        .setup
                        .group_rect(u64::from(tile.group_y) * columns + u64::from(tile.group_x))?;
                    let last = self.setup.group_rect(
                        u64::from(tile.group_y + tile.groups_high - 1) * columns
                            + u64::from(tile.group_x + tile.groups_wide - 1),
                    )?;
                    crate::trace::region(
                        u64::from(first.x()),
                        u64::from(first.y()),
                        u64::from(last.x() + last.width() - first.x()),
                        u64::from(last.y() + last.height() - first.y()),
                    );
                }
                let plan = if self.setup.exact.is_some() {
                    Some(Arc::new(ExactPlan::read(
                        &self.setup,
                        tile,
                        exact_plan,
                        self.session,
                    )?))
                } else {
                    if !exact_plan.is_empty() {
                        return Err(Error::Corrupt("V4 exact plan without exact plane"));
                    }
                    None
                };
                self.groups = Some(self.setup.layout.groups(id)?);
                self.current = self.groups.as_mut().and_then(Iterator::next);
                self.group_ordinal = 0;
                self.phase = 0;
                self.exact_next = None;
                self.exact = plan.clone();
                self.tile = Some(id);
                Ok(LossyEvent::Begin {
                    tile,
                    exact: plan,
                    entry: located(extent, EntryKind::TileBegin, 0, id),
                })
            }
            TileEvent::Object { extent, object } => {
                if object.kind & 1 == 0 {
                    let ordinal = self.ignorable;
                    self.ignorable = self.ignorable.checked_add(1).ok_or(Error::TooLarge)?;
                    return Ok(LossyEvent::Ancillary {
                        kind: object.kind,
                        body: object.body,
                        entry: located(extent, EntryKind::Ignorable, 0, ordinal),
                    });
                }
                let mut r = ByteReader::new(object.body);
                if object.kind == crate::registry::MODEL_DEFINITION {
                    let role =
                        u8::try_from(r.varint()?).map_err(|_| Error::Corrupt("V4 model role"))?;
                    let id = r.varint()?;
                    let kind = EntryKind::model_lifetime(r.varint()?)?;
                    if let Some(resolver) = resolver {
                        resolver.define(object.body)?;
                    } else {
                        self.models.define(object.body)?;
                    }
                    return Ok(LossyEvent::Definition {
                        entry: located(extent, kind, role, id),
                    });
                }
                let role = match object.kind {
                    crate::registry::UNIT_DC => 0,
                    crate::registry::UNIT_AC => 1,
                    crate::registry::UNIT_ALPHA => 2,
                    crate::registry::UNIT_EXACT => 3,
                    _ => return Err(Error::Unsupported("unknown V4 lossy critical object")),
                };
                let id = r.varint()?;
                if self.expected()? != Some((role, id)) {
                    return Err(Error::Corrupt("V4 lossy main schedule/identity"));
                }
                let work = if let Some(resolver) = resolver {
                    super::read_unit(Arc::clone(&self.setup), role, object.body, resolver)?
                } else {
                    self.models.unit(role, object.body)?
                };
                if role == 3 {
                    self.exact_next.as_mut().unwrap().start += 1;
                } else {
                    self.phase += 1;
                }
                Ok(LossyEvent::Unit {
                    work,
                    entry: located(extent, EntryKind::Unit, role, id),
                })
            }
            TileEvent::End {
                extent,
                tile,
                pixel_results,
            } => {
                if self.expected()?.is_some() {
                    return Err(Error::Corrupt("V4 tile closes before its lossy groups"));
                }
                self.groups = None;
                self.current = None;
                self.exact = None;
                self.tile = None;
                Ok(LossyEvent::End {
                    pixel_results,
                    tile,
                    entry: located(extent, EntryKind::TileEnd, 0, tile.tile_id),
                })
            }
        }
    }

    pub fn finish_structure(&self) -> Result<(), Error> {
        if self.failed || self.tile.is_some() {
            return Err(Error::Corrupt("incomplete V4 lossy object stream"));
        }
        self.frames.finish()?;
        self.models.finish()
    }
}
