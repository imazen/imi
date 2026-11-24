//! Complete-slice group descriptors, with compressed spans instead of rasters.
//!
//! A row consumer orders groups geometrically even when tiles are stored in
//! Morton order. Descriptor capacity and models are admitted before retention;
//! only the current row band and its numeric dependencies are reconstructed.

use alloc::{borrow::Cow, boxed::Box, sync::Arc, vec::Vec};

use super::{LossyEvent, LossyFile, NumericFile, TileLayout, TileLimits};
use crate::decode_resources::{DecodeSession, V4Resume, V4Retained};
use crate::lossy::v4::{NumericDependency, StoredLossyUnit, prepare_group};
use crate::parse::limits::Admission;
use crate::planes::Rect;
use crate::{Error, ImageInfo};

struct ExactSource {
    plan: Arc<super::ExactPlan>,
    stripes: Vec<StoredLossyUnit>,
}

struct LossyGroup {
    unit: StoredLossyUnit,
    dc: Arc<StoredLossyUnit>,
    alpha: Option<StoredLossyUnit>,
    exact: Option<Arc<ExactSource>>,
    ordinal: u64,
    tile: u64,
}

struct Group {
    work: Box<LossyGroup>,
}
impl Group {
    fn id(&self) -> u64 {
        self.work.unit.id()
    }
    fn rect(&self) -> Result<Rect, Error> {
        self.work.unit.rect()
    }
}
struct NumericPlan {
    model: Arc<super::NumericModel>,
    // Group ordinal is the vector index. No model handle or rectangle per group.
    entropy: Vec<core::ops::Range<usize>>,
}

struct PlanChecks {
    checks: Arc<super::OwnedChecks>,
    tags: Vec<core::ops::Range<usize>>,
}
impl PlanChecks {
    fn new<C: Fn() -> bool + Sync>(
        checks: Option<&Arc<super::OwnedChecks>>,
        layout: TileLayout,
        session: &DecodeSession<C>,
    ) -> Result<Option<Self>, Error> {
        if !session.v4_pixel_checks(super::CheckScope::TilePixels) {
            return Ok(None);
        }
        let Some(checks) = checks.filter(|c| c.width(super::CheckScope::TilePixels) != 0) else {
            return Ok(None);
        };
        let count = usize::try_from(layout.tile_count()).map_err(|_| Error::TooLarge)?;
        session.v4_models().hint(
            count
                .checked_mul(core::mem::size_of::<core::ops::Range<usize>>())
                .ok_or(Error::TooLarge)?,
        )?;
        let mut tags = Vec::new();
        tags.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
        tags.resize(count, 0..0);
        Ok(Some(Self {
            checks: checks.clone(),
            tags,
        }))
    }
    fn record(&mut self, id: u64, expected: &[u8], input: &[u8]) -> Result<(), Error> {
        let range = crate::owned::span(input, expected)?;
        let slot = self
            .tags
            .get_mut(usize::try_from(id).map_err(|_| Error::TooLarge)?)
            .ok_or(Error::Corrupt("V4 tile check plan identity"))?;
        if slot.start != slot.end {
            return Err(Error::Corrupt("duplicate V4 tile check plan"));
        }
        *slot = range;
        Ok(())
    }
}

pub(crate) struct Plan {
    info: ImageInfo,
    header: crate::Header,
    metadata: crate::Metadata,
    layout: TileLayout,
    groups: Vec<Group>,
    numeric: Option<NumericPlan>,
    tile_checks: Option<PlanChecks>,
    _charge: V4Retained,
}

fn push<T, C: Fn() -> bool + Sync>(
    values: &mut Vec<T>,
    value: T,
    session: &DecodeSession<C>,
) -> Result<(), Error> {
    session.v4_reserve(values, 1)?;
    values.push(value);
    Ok(())
}

fn reserve_groups<C: Fn() -> bool + Sync>(
    groups: &mut Vec<Group>,
    layout: TileLayout,
    session: &DecodeSession<C>,
) -> Result<(), Error> {
    let count = usize::try_from(layout.group_count()).map_err(|_| Error::TooLarge)?;
    let bytes = count
        .checked_mul(core::mem::size_of::<Group>())
        .ok_or(Error::TooLarge)?;
    session.v4_models().hint(bytes)?;
    groups.try_reserve_exact(count).map_err(|_| Error::TooLarge)
}

impl Plan {
    fn read<C: Fn() -> bool + Sync>(
        input: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<
        (
            Self,
            Option<crate::pixelsum::RowCheck>,
            crate::PixelChecksumStatus,
        ),
        Error,
    > {
        let scope = session.v4_model_scope();
        let parsed = super::file::parse_file(input, session)?;
        let header = parsed.header.clone();
        session.v4_output(&header.info)?;
        let limits = TileLimits {
            max_object_bytes: u64::MAX,
            max_tile_bytes: u64::MAX,
        };
        let mut groups = Vec::new();
        let mut numeric = None;
        let mut tile_checks;
        let (layout, table, completed) = match header.coding {
            crate::Coding::Lossless => {
                let file = NumericFile::from_parsed(parsed, input.len(), limits, session)?;
                let layout = file.layout();
                let table = file.pixelsum();
                tile_checks = PlanChecks::new(file.check_config(), layout, session)?;
                let count = usize::try_from(layout.group_count()).map_err(|_| Error::TooLarge)?;
                session.v4_models().hint(
                    count
                        .checked_mul(core::mem::size_of::<core::ops::Range<usize>>())
                        .ok_or(Error::TooLarge)?,
                )?;
                let mut entropy = Vec::new();
                entropy
                    .try_reserve_exact(count)
                    .map_err(|_| Error::TooLarge)?;
                entropy.resize(count, 0..0);
                let mut model = None;
                let completed = file.visit_checked(|event| {
                    let work = match event {
                        super::decode::ImageEvent::Group(super::decode::GroupWork::Numeric(
                            work,
                        )) => work,
                        super::decode::ImageEvent::Tile(tile) => {
                            if let Some(checks) = &mut tile_checks {
                                checks.record(tile.id, tile.expected, input)?;
                            }
                            return Ok(());
                        }
                        _ => return Err(Error::Corrupt("V4 numeric plan work")),
                    };
                    let (shared, id, span) = work.store(input)?;
                    let slot = entropy
                        .get_mut(usize::try_from(id).map_err(|_| Error::TooLarge)?)
                        .ok_or(Error::Corrupt("V4 numeric plan identity"))?;
                    if slot.start != 0 {
                        return Err(Error::Corrupt("duplicate V4 planned group"));
                    }
                    *slot = span;
                    model.get_or_insert(shared);
                    Ok(())
                })?;
                if entropy.iter().any(|span| span.start == 0) {
                    return Err(Error::Corrupt("V4 numeric plan coverage"));
                }
                session.adapter_work(
                    count as u128 * core::mem::size_of::<core::ops::Range<usize>>() as u128,
                )?;
                numeric = Some(NumericPlan {
                    model: model.ok_or(Error::Corrupt("V4 numeric plan model"))?,
                    entropy,
                });
                (layout, table, completed)
            }
            crate::Coding::Lossy => {
                let file = LossyFile::from_parsed(parsed, input.len(), limits, session)?;
                let layout = file.layout();
                let table = file.pixelsum();
                tile_checks = PlanChecks::new(file.check_config(), layout, session)?;
                reserve_groups(&mut groups, layout, session)?;
                let mut dc: Vec<Option<Arc<StoredLossyUnit>>> = Vec::new();
                let mut alpha = None;
                let mut exact = None;
                let mut stripes = Vec::new();
                let mut tile_start = 0;
                let mut tile_x = 0;
                let mut tile_id = 0;
                let mut ordinal = 0;
                let completed = file.visit_units(|event| {
                    match event {
                        LossyEvent::Begin {
                            tile, exact: plan, ..
                        } => {
                            dc.clear();
                            let columns = tile.groups_wide as usize;
                            // The maximum column cache remains charged once.
                            if columns > dc.capacity() {
                                session.v4_models().hint(
                                    (columns - dc.capacity())
                                        .checked_mul(core::mem::size_of::<
                                            Option<Arc<StoredLossyUnit>>,
                                        >())
                                        .ok_or(Error::TooLarge)?,
                                )?;
                                dc.try_reserve_exact(columns).map_err(|_| Error::TooLarge)?;
                            }
                            dc.resize_with(columns, || None);
                            tile_x = tile.group_x;
                            tile_id = tile.id;
                            tile_start = groups.len();
                            ordinal = 0;
                            exact = plan;
                            stripes = Vec::new();
                        }
                        LossyEvent::Unit { work, .. } => {
                            let id = work.id();
                            let role = work.role();
                            let column = (id % u64::from(layout.group_grid().0))
                                .checked_sub(u64::from(tile_x))
                                .ok_or(Error::Corrupt("V4 plan column"))?;
                            let stored = work.store(input)?;
                            match role {
                                0 => {
                                    session.v4_models().hint(
                                        core::mem::size_of::<StoredLossyUnit>()
                                            + 2 * core::mem::size_of::<usize>(),
                                    )?;
                                    *dc.get_mut(column as usize)
                                        .ok_or(Error::Corrupt("V4 plan DC column"))? =
                                        Some(Arc::new(stored));
                                }
                                2 => alpha = Some(stored),
                                1 => {
                                    let dc = Arc::clone(
                                        dc.get(column as usize)
                                            .and_then(Option::as_ref)
                                            .ok_or(Error::Corrupt("V4 plan missing DC"))?,
                                    );
                                    session
                                        .v4_models()
                                        .hint(core::mem::size_of::<LossyGroup>())?;
                                    push(
                                        &mut groups,
                                        Group {
                                            work: Box::new(LossyGroup {
                                                unit: stored,
                                                dc,
                                                alpha: alpha.take(),
                                                exact: None,
                                                ordinal,
                                                tile: tile_id,
                                            }),
                                        },
                                        session,
                                    )?;
                                    ordinal += 1;
                                }
                                3 => push(&mut stripes, stored, session)?,
                                _ => return Err(Error::Corrupt("V4 plan unit role")),
                            }
                        }
                        LossyEvent::End {
                            tile,
                            pixel_results,
                            ..
                        } => {
                            if let Some(checks) = &mut tile_checks {
                                checks.record(tile.tile_id, pixel_results, input)?;
                            }
                            if let Some(plan) = exact.take() {
                                if plan.stripe_count() != stripes.len() as u64 {
                                    return Err(Error::Corrupt("V4 plan exact stripe count"));
                                }
                                session.v4_models().hint(
                                    core::mem::size_of::<ExactSource>()
                                        + 2 * core::mem::size_of::<usize>(),
                                )?;
                                let source = Arc::new(ExactSource {
                                    plan,
                                    stripes: core::mem::take(&mut stripes),
                                });
                                for group in &mut groups[tile_start..] {
                                    group.work.exact = Some(Arc::clone(&source));
                                }
                            }
                            dc.clear();
                        }
                        _ => {}
                    }
                    Ok(())
                })?;
                (layout, table, completed)
            }
        };
        session.adapter_work(groups.len() as u128 * 64)?;
        groups.sort_unstable_by_key(Group::id);
        if numeric.is_none()
            && (groups.len() as u64 != layout.group_count()
                || groups
                    .iter()
                    .enumerate()
                    .any(|(id, group)| group.id() != id as u64))
        {
            return Err(Error::Corrupt("V4 plan group coverage"));
        }
        if matches!(table, crate::pixelsum::Table::Known(..)) {
            session.admission().pixel_checksum(&header.info)?;
        }
        let checksum = crate::pixelsum::RowCheck::new(table, &header.info)?;
        let charge = scope.commit()?;
        Ok((
            Self {
                info: header.info,
                header: completed.0,
                metadata: completed.1,
                layout,
                groups,
                numeric,
                tile_checks,
                _charge: charge,
            },
            checksum,
            table.status(),
        ))
    }

    #[cfg(feature = "std")]
    fn len(&self) -> usize {
        self.numeric
            .as_ref()
            .map_or(self.groups.len(), |p| p.entropy.len())
    }
    fn rect(&self, id: usize) -> Result<Rect, Error> {
        if let Some(numeric) = &self.numeric {
            return numeric.model.group_rect(id as u64);
        }
        self.groups
            .get(id)
            .ok_or(Error::Corrupt("V4 planned group"))?
            .rect()
    }
    fn tile(&self, id: usize) -> Option<u64> {
        self.groups.get(id).map(|group| group.work.tile)
    }

    /// Admit reconstruction from known descriptors before a complete-input
    /// consumer can publish pixels. Only one full-width group band is retained;
    /// model/descriptor charges already belong to this plan. Dependency storage
    /// includes column caches, exact stripe overlap and the selected exact target.
    fn execution_bounds(&self, input: &[u8]) -> Result<(u128, u128), Error> {
        use crate::lossy::v4::plane_capacity;
        let mut work = 0u128;
        let mut band = 0u128;
        let mut scratch = 0u128;
        let mut peak = 0u128;
        let mut y = None;
        if let Some(numeric) = &self.numeric {
            for id in 0..numeric.entropy.len() {
                let rect = numeric.model.group_rect(id as u64)?;
                let (bytes, steps) = numeric.model.execution_cost(rect)?;
                peak = peak.max(self.info.row_bytes() as u128 * u128::from(rect.height()) + bytes);
                work += steps;
            }
            return Ok((peak, work));
        }
        for group in &self.groups {
            let rect = group.rect()?;
            if y != Some(rect.y()) {
                peak = peak.max(band + scratch);
                band = self.info.row_bytes() as u128 * u128::from(rect.height());
                scratch = 0;
                y = Some(rect.y());
            }
            {
                let source = &group.work;
                let LossyGroup {
                    unit,
                    dc,
                    alpha,
                    exact,
                    ordinal,
                    ..
                } = &**source;
                let unit = unit.bind(input)?;
                let dc = dc.bind(input)?;
                let (_, dc_rect) = unit.setup().dc_for_group(unit.id())?;
                let geometry = dc
                    .setup()
                    .numeric_geometry(0)
                    .ok_or(Error::Corrupt("V4 DC geometry"))?;
                band += plane_capacity(
                    &geometry.info,
                    Rect::new(0, 0, dc_rect.width(), geometry.stripe_rows),
                );
                let (bytes, steps) = dc.numeric_execution_cost(dc_rect)?;
                scratch = scratch.max(bytes);
                // Repeated references are a conservative work reservation;
                // execution may reuse the immutable decoded DC cache.
                work += steps;
                if let Some(source) = alpha {
                    let source = source.bind(input)?;
                    let geometry = source
                        .setup()
                        .numeric_geometry(2)
                        .ok_or(Error::Corrupt("V4 alpha geometry"))?;
                    band += plane_capacity(&geometry.info, rect);
                    let (bytes, steps) = source.numeric_execution_cost(rect)?;
                    scratch = scratch.max(bytes);
                    work += steps;
                }
                if let Some(source) = exact {
                    if let Some(stripe) = source.stripes.first() {
                        let stripe = stripe.bind(input)?;
                        let geometry = stripe
                            .setup()
                            .numeric_geometry(3)
                            .ok_or(Error::Corrupt("V4 exact geometry"))?;
                        band += 2 * plane_capacity(&geometry.info, source.plan.stripe_rect(0)?);
                    }
                    let blocks = source.plan.group_blocks(*ordinal)?;
                    let count = blocks.end - blocks.start;
                    work += u128::from(rect.width().div_ceil(8))
                        * u128::from(rect.height().div_ceil(8));
                    if count != 0 {
                        let height = u32::try_from(count.checked_mul(8).ok_or(Error::TooLarge)?)
                            .map_err(|_| Error::TooLarge)?;
                        let info =
                            ImageInfo::new(8, height, crate::Channels::Rgb, crate::SampleType::U8);
                        band += plane_capacity(&info, Rect::new(0, 0, 8, height));
                        let per = source.plan.blocks_per_stripe();
                        for id in blocks.start / per..blocks.end.div_ceil(per) {
                            let stripe = source
                                .stripes
                                .get(id as usize)
                                .ok_or(Error::Corrupt("V4 exact dependency"))?
                                .bind(input)?;
                            let rect = source.plan.stripe_rect(id)?;
                            let (bytes, steps) = stripe.numeric_execution_cost(rect)?;
                            scratch = scratch.max(bytes);
                            work += steps;
                        }
                    }
                }
                let (bytes, steps) = unit.ac_execution_cost()?;
                scratch = scratch.max(bytes);
                work += steps;
            }
        }
        peak = peak.max(band + scratch);
        Ok((peak, work))
    }

    #[allow(clippy::too_many_arguments)]
    fn decode<C: Fn() -> bool + Sync>(
        &self,
        id: usize,
        input: &[u8],
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
        dc_cache: &mut NumericDependency,
        exact_cache: &mut NumericDependency,
    ) -> Result<(), Error> {
        if let Some(numeric) = &self.numeric {
            let span = numeric
                .entropy
                .get(id)
                .ok_or(Error::Corrupt("V4 planned group"))?;
            return numeric
                .model
                .decode_group(
                    id as u64,
                    input
                        .get(span.clone())
                        .ok_or(Error::Corrupt("V4 stored numeric span"))?,
                    output,
                    stride,
                    session,
                )
                .map(|_| ());
        }
        let group = self
            .groups
            .get(id)
            .ok_or(Error::Corrupt("V4 planned group"))?;
        {
            let source = &group.work;
            let LossyGroup {
                unit,
                dc,
                alpha,
                exact,
                ordinal,
                ..
            } = &**source;
            let work = prepare_group(
                unit.bind(input)?,
                dc.bind(input)?,
                alpha.as_ref().map(|unit| unit.bind(input)).transpose()?,
                exact.as_ref().map(|source| (&*source.plan, *ordinal)),
                |id| {
                    exact
                        .as_ref()
                        .and_then(|source| source.stripes.get(id as usize))
                        .ok_or(Error::Corrupt("V4 planned exact dependency"))?
                        .bind(input)
                },
                session,
                dc_cache,
                exact_cache,
            )?;
            work.decode_into(output, stride, session)?;
        }
        Ok(())
    }
}

/// Own or borrow compressed input without self-referential model slices.
pub(crate) struct Rows<'a> {
    input: Cow<'a, [u8]>,
    plan: Plan,
    budget: V4Resume,
    y: u32,
    band_y: u32,
    band_rows: u32,
    band: Vec<u8>,
    band_charge: V4Retained,
    _state_charge: V4Retained,
    next_group: usize,
    dc_cache: Vec<NumericDependency>,
    exact_cache: Vec<(Option<u64>, NumericDependency)>,
    tile_hashes: Vec<Option<super::RetainedPixelCheckState>>,
}

impl<'a> Rows<'a> {
    pub(crate) fn new<C: Fn() -> bool + Sync>(
        input: Cow<'a, [u8]>,
        session: &DecodeSession<C>,
    ) -> Result<
        (
            Self,
            Option<crate::pixelsum::RowCheck>,
            crate::PixelChecksumStatus,
        ),
        Error,
    > {
        let scope = session.v4_model_scope();
        session.v4_models().hint(
            core::mem::size_of::<Self>()
                + core::mem::size_of::<crate::decode_resources::AdmittedRows<'a>>()
                + core::mem::size_of::<crate::pixelsum::Raster>(),
        )?;
        let (plan, checksum, status) = Plan::read(&input, session)?;
        let dc_count = plan.layout.group_grid().0 as usize;
        let tile_count = plan.layout.tile_grid().0 as usize;
        session.v4_models().hint(
            dc_count
                .checked_mul(core::mem::size_of::<NumericDependency>())
                .and_then(|n| {
                    tile_count
                        .checked_mul(core::mem::size_of::<(Option<u64>, NumericDependency)>())
                        .and_then(|m| n.checked_add(m))
                })
                .ok_or(Error::TooLarge)?,
        )?;
        let mut dc_cache = Vec::new();
        dc_cache
            .try_reserve_exact(dc_count)
            .map_err(|_| Error::TooLarge)?;
        dc_cache.resize_with(dc_count, NumericDependency::default);
        let mut exact_cache = Vec::new();
        exact_cache
            .try_reserve_exact(tile_count)
            .map_err(|_| Error::TooLarge)?;
        exact_cache.resize_with(tile_count, || (None, NumericDependency::default()));
        let mut tile_hashes = Vec::new();
        if plan.tile_checks.is_some() {
            session.v4_models().hint(
                tile_count
                    .checked_mul(core::mem::size_of::<Option<super::RetainedPixelCheckState>>())
                    .ok_or(Error::TooLarge)?,
            )?;
            tile_hashes
                .try_reserve_exact(tile_count)
                .map_err(|_| Error::TooLarge)?;
            tile_hashes.resize_with(tile_count, || None);
        }
        let (peak, work) = plan.execution_bounds(&input)?;
        drop(session.v4_working(peak)?);
        session.v4_reserve_work(work)?;
        let state_charge = scope.commit()?;
        let budget = session.v4_resume();
        Ok((
            Self {
                input,
                plan,
                budget,
                y: 0,
                band_y: 0,
                band_rows: 0,
                band: Vec::new(),
                band_charge: V4Retained::default(),
                _state_charge: state_charge,
                next_group: 0,
                dc_cache,
                exact_cache,
                tile_hashes,
            },
            checksum,
            status,
        ))
    }
    pub(crate) fn info(&self) -> &ImageInfo {
        &self.plan.info
    }
    pub(crate) fn description(&self) -> (crate::Header, crate::Metadata) {
        (self.plan.header.clone(), self.plan.metadata.clone())
    }
    pub(crate) fn rows_decoded(&self) -> u32 {
        self.y
    }
    pub(crate) fn usage(&self) -> crate::decode_resources::DecodeUsage {
        self.budget.usage()
    }
    pub(crate) fn resume(&self) -> V4Resume {
        let session = self.budget.session(|| false);
        session.v4_resume()
    }
    pub(crate) fn progress<'p>(
        &mut self,
        pulse: Option<&'p dyn how_far::Pulse>,
    ) -> Result<(Option<crate::progress::UnitProgress<'p>>, V4Retained), Error> {
        #[cfg(feature = "std")]
        {
            use how_far::ProgressExt;
            if let Some(pulse) = pulse.filter(|pulse| pulse.live().is_some()) {
                let session = self.budget.session(|| false);
                let result = (|| {
                    let scope = session.v4_model_scope();
                    let count = self.plan.len();
                    session
                        .v4_models()
                        .hint(count.checked_mul(1024).ok_or(Error::TooLarge)?)?;
                    let (peak, _) = self.plan.execution_bounds(&self.input)?;
                    drop(session.v4_working(peak)?);
                    let columns = self.plan.layout.group_grid().0 as usize;
                    let mut shapes = Vec::new();
                    shapes
                        .try_reserve_exact(count)
                        .map_err(|_| Error::TooLarge)?;
                    for id in 0..count {
                        let rect = self.plan.rect(id)?;
                        shapes.push(crate::progress::UnitShape {
                            stripe: id / columns,
                            group: id % columns,
                            groups: columns,
                            y: rect.y() as usize,
                            rows: rect.height() as usize,
                            x: rect.x() as usize,
                            cols: rect.width() as usize,
                        });
                    }
                    Ok((
                        crate::progress::UnitProgress::plan(pulse, shapes.into_iter(), 1),
                        scope.commit()?,
                    ))
                })();
                self.budget.update(&session);
                return result;
            }
        }
        #[cfg(not(feature = "std"))]
        let _ = pulse;
        Ok((None, V4Retained::default()))
    }
    pub(crate) fn next_row_checked(
        &mut self,
        output: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        self.next_row_observed(output, check, None)
    }
    pub(crate) fn next_row_observed(
        &mut self,
        output: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
        progress: Option<&crate::progress::UnitProgress<'_>>,
    ) -> Result<bool, Error> {
        check()?;
        let row = self.plan.info.row_bytes();
        if output.len() < row {
            return Err(Error::InvalidInput("row buffer too small"));
        }
        if self.y == self.plan.info.height {
            return Ok(false);
        }
        if self.band.is_empty() || self.y == self.band_y + self.band_rows {
            self.band = Vec::new();
            self.band_charge = V4Retained::default();
            let session = self.budget.session(|| check().is_err());
            let result = (|| {
                let rect = self.plan.rect(self.next_group)?;
                if rect.x() != 0 || rect.y() != self.y {
                    return Err(Error::Corrupt("V4 row band origin"));
                }
                let bytes = row
                    .checked_mul(rect.height() as usize)
                    .ok_or(Error::TooLarge)?;
                let scope = session.v4_model_scope();
                session.v4_models().hint(bytes)?;
                self.band =
                    bytemuck::allocation::try_zeroed_vec(bytes).map_err(|_| Error::TooLarge)?;
                self.band_charge = scope.commit()?;
                self.band_y = rect.y();
                self.band_rows = rect.height();
                for _ in 0..self.plan.layout.group_grid().0 {
                    let current = self.plan.rect(self.next_group)?;
                    if current.y() != rect.y() || current.height() != rect.height() {
                        return Err(Error::Corrupt("V4 row band geometry"));
                    }
                    let at = current.x() as usize * self.plan.info.bytes_per_pixel();
                    let column = self.next_group % self.plan.layout.group_grid().0 as usize;
                    let tile_column = column / self.plan.layout.tile_shape().0 as usize;
                    let exact = &mut self.exact_cache[tile_column];
                    if let Some(tile) = self.plan.tile(self.next_group)
                        && exact.0 != Some(tile)
                    {
                        exact.1.clear();
                        exact.0 = Some(tile);
                    }
                    if let Some(progress) = progress {
                        progress.begin(self.next_group)?;
                    }
                    let decoded = self.plan.decode(
                        self.next_group,
                        &self.input,
                        &mut self.band[at..],
                        row,
                        &session,
                        &mut self.dc_cache[column],
                        &mut exact.1,
                    );
                    if let Some(progress) = progress {
                        progress.end(self.next_group, &decoded);
                    }
                    decoded?;
                    self.next_group += 1;
                }
                if let Some(checks) = &self.plan.tile_checks {
                    let (columns, _) = self.plan.layout.group_grid();
                    let (tile_columns, _) = self.plan.layout.tile_shape();
                    for column in 0..self.plan.layout.tile_grid().0 {
                        let group_id = (self.next_group - columns as usize) as u64
                            + u64::from(column * tile_columns);
                        let tile = self.plan.layout.tile_for_group(group_id)?;
                        let rect = super::tile_pixel_rect(self.plan.layout, tile.id, |id| {
                            self.plan.rect(id as usize)
                        })?;
                        let mut local = self.plan.info.clone();
                        local.width = rect.width();
                        local.height = rect.height();
                        let _scratch =
                            session.v4_working(super::PixelChecks::SCRATCH_BYTES as u128)?;
                        let list = checks.checks.get().list(super::CheckScope::TilePixels);
                        let (mut hash, charge) = super::PixelChecks::tile_state(
                            list,
                            self.tile_hashes[column as usize].take(),
                            super::PixelCheckIdentity {
                                image: 0,
                                id: tile.id,
                                x: rect.x(),
                                y: rect.y(),
                            },
                            &local,
                            &session,
                        )?;
                        local.height = self.band_rows;
                        let at = rect.x() as usize * self.plan.info.bytes_per_pixel();
                        let known = list.iter().filter(|c| c.algorithm().is_some()).count();
                        session.adapter_work(
                            u128::from(local.width)
                                * u128::from(local.height)
                                * local.bytes_per_pixel() as u128
                                * known as u128,
                        )?;
                        hash.rows_checked(
                            crate::ImageRef::new(&self.band[at..], row, &local),
                            || session.v4_models().check(),
                        )?;
                        if self.band_y + self.band_rows == rect.y() + rect.height() {
                            let expected = self
                                .input
                                .get(checks.tags[tile.id as usize].clone())
                                .ok_or(Error::Corrupt("V4 planned tile tag span"))?;
                            hash.verify(expected)?;
                        } else {
                            self.tile_hashes[column as usize] = Some((hash.into_state(), charge));
                        }
                    }
                }
                Ok(())
            })();
            self.budget.update(&session);
            result?;
        }
        check()?;
        let at = (self.y - self.band_y) as usize * row;
        output[..row].copy_from_slice(&self.band[at..at + row]);
        self.y += 1;
        Ok(true)
    }
}
