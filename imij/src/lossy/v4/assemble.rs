//! Decoded dependency ownership and independently schedulable lossy work.

use alloc::{sync::Arc, vec::Vec};
use core::ops::Range;

use super::{ExactPlan, LossyEvent, LossyGroupInputs, LossySetup, LossyUnit};
use crate::decode_resources::{DecodeSession, V4Retained};
use crate::parse::limits::Admission;
use crate::planes::Rect;
use crate::v4::SpatialTile;
use crate::{Channels, Error, ImageInfo, ImageRef, SampleType};

struct Plane {
    info: ImageInfo,
    rect: Rect,
    pixels: Vec<u8>,
    stride: usize,
    _charge: V4Retained,
}

pub(crate) fn plane_capacity(info: &ImageInfo, rect: Rect) -> u128 {
    u128::from(rect.width()) * u128::from(rect.height()) * info.bytes_per_pixel() as u128
        + (core::mem::size_of::<Plane>() + 2 * core::mem::size_of::<usize>()) as u128
}

impl Plane {
    fn empty<C: Fn() -> bool + Sync>(
        info: &ImageInfo,
        rect: Rect,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        let mut local = info.clone();
        local.width = rect.width();
        local.height = rect.height();
        let count = local.checked_buffer_len().ok_or(Error::TooLarge)?;
        let stride = (local.width as usize)
            .checked_mul(local.bytes_per_pixel())
            .ok_or(Error::TooLarge)?;
        session.v4_models().hint(
            count
                .checked_add(core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>())
                .ok_or(Error::TooLarge)?,
        )?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        pixels.resize(count, 0);
        Ok(Self {
            info: local,
            rect,
            pixels,
            stride,
            _charge: scope.commit()?,
        })
    }

    fn decode<C: Fn() -> bool + Sync>(
        work: &LossyUnit<'_>,
        rect: Rect,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let geometry = work
            .setup()
            .numeric_geometry(u64::from(work.role()))
            .ok_or(Error::Corrupt("V4 numeric geometry missing"))?;
        let mut plane = Self::empty(&geometry.info, rect, session)?;
        work.decode_numeric_into(rect, &mut plane.pixels, plane.stride, session)?;
        Ok(plane)
    }

    fn view(&self) -> ImageRef<'_> {
        ImageRef::new(&self.pixels, self.stride, &self.info)
    }
}

/// AC work with owned, immutable decoded dependencies. It can remain queued
/// while the source parses/fetches later tiles. Queued dependency capacity stays
/// charged until its final owner drops; worker scratch is admitted separately.
pub struct LossyWork<'a> {
    unit: LossyUnit<'a>,
    dc: Arc<Plane>,
    alpha: Option<Arc<Plane>>,
    exact: Option<Arc<Plane>>,
}

impl LossyWork<'_> {
    pub(crate) fn id(&self) -> u64 {
        self.unit.id()
    }
    pub fn rect(&self) -> Result<Rect, Error> {
        self.unit.setup().group_rect(self.unit.id())
    }

    pub fn decode_into<C: Fn() -> bool + Sync>(
        &self,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<Rect, Error> {
        self.decode_with_reference(output, stride, None, session)
    }

    /// An optional reference is a group-local native view, with this group's
    /// dimensions and source interpretation. It is needed only for kept pixels.
    pub fn decode_with_reference<C: Fn() -> bool + Sync>(
        &self,
        output: &mut [u8],
        stride: usize,
        reference: Option<ImageRef<'_>>,
        session: &DecodeSession<C>,
    ) -> Result<Rect, Error> {
        let rect = self.rect()?;
        let (_, expected) = self.unit.setup().dc_for_group(self.unit.id())?;
        if self.dc.rect != expected {
            return Err(Error::Corrupt("V4 stale DC dependency"));
        }
        let y = (rect.y() / 8)
            .checked_sub(self.dc.rect.y())
            .ok_or(Error::Corrupt("V4 DC row origin"))?;
        let offset = (y as usize)
            .checked_mul(self.dc.stride)
            .ok_or(Error::TooLarge)?;
        let mut info = self.dc.info.clone();
        info.width = rect.width().div_ceil(8);
        info.height = rect.height().div_ceil(8);
        self.unit.decode_into(
            LossyGroupInputs {
                dc: ImageRef::new(&self.dc.pixels[offset..], self.dc.stride, &info),
                alpha: self.alpha.as_ref().map(|p| p.view()),
                exact: self.exact.as_ref().map(|p| p.view()),
                reference,
            },
            output,
            stride,
            session,
        )
    }
}

fn exact_count<C: Fn() -> bool + Sync>(
    setup: &LossySetup,
    id: u64,
    dc: &Plane,
    session: &DecodeSession<C>,
) -> Result<u64, Error> {
    let rect = setup.group_rect(id)?;
    if dc.rect != setup.dc_for_group(id)?.1 {
        return Err(Error::Corrupt("V4 exact stale DC coverage"));
    }
    let start = rect.y() / 8 - dc.rect.y();
    let bps = dc.info.sample.bytes();
    let mut count = 0;
    session.adapter_work(
        u128::from(rect.width().div_ceil(8)) * u128::from(rect.height().div_ceil(8)),
    )?;
    for y in 0..rect.height().div_ceil(8) as usize {
        for x in 0..rect.width().div_ceil(8) as usize {
            let at = (start as usize + y) * dc.stride + (x * 4 + 3) * bps;
            let code = if dc.info.sample == SampleType::U16 {
                u16::from_ne_bytes(dc.pixels[at..at + 2].try_into().unwrap()) as i64
            } else {
                i32::from_ne_bytes(dc.pixels[at..at + 4].try_into().unwrap()) as i64
            };
            match code {
                0 => {}
                1 => count += 1,
                _ => return Err(Error::Corrupt("V4 exact block code")),
            }
        }
    }
    Ok(count)
}

#[derive(Default)]
pub(crate) struct NumericDependency {
    id: Option<u64>,
    pixels: Option<Arc<Plane>>,
}
impl NumericDependency {
    fn get<C: Fn() -> bool + Sync>(
        &mut self,
        source: &LossyUnit<'_>,
        rect: Rect,
        session: &DecodeSession<C>,
    ) -> Result<Arc<Plane>, Error> {
        if self.id != Some(source.id()) {
            self.pixels = None;
            self.id = None;
            self.pixels = Some(Arc::new(Plane::decode(source, rect, session)?));
            self.id = Some(source.id());
        }
        Ok(Arc::clone(
            self.pixels
                .as_ref()
                .ok_or(Error::Corrupt("V4 dependency cache"))?,
        ))
    }
    pub(crate) fn clear(&mut self) {
        self.id = None;
        self.pixels = None;
    }
}

/// Bind just one addressed group's numeric dependencies. The raw units have
/// already passed the owning physical schedule; no earlier AC is decoded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_group<'a, C: Fn() -> bool + Sync>(
    unit: LossyUnit<'a>,
    dc: LossyUnit<'a>,
    alpha: Option<LossyUnit<'a>>,
    exact: Option<(&ExactPlan, u64)>,
    mut stripe: impl FnMut(u64) -> Result<LossyUnit<'a>, Error>,
    session: &DecodeSession<C>,
    dc_cache: &mut NumericDependency,
    exact_cache: &mut NumericDependency,
) -> Result<LossyWork<'a>, Error> {
    let setup = unit.shared_setup();
    let (dc_id, rect) = setup.dc_for_group(unit.id())?;
    if unit.role() != 1 || dc.role() != 0 || dc.id() != dc_id {
        return Err(Error::Corrupt("V4 addressed DC identity"));
    }
    let dc = dc_cache.get(&dc, rect, session)?;
    let alpha = alpha
        .map(|source| {
            if source.role() != 2 || source.id() != unit.id() {
                return Err(Error::Corrupt("V4 addressed alpha identity"));
            }
            Plane::decode(&source, setup.group_rect(unit.id())?, session).map(Arc::new)
        })
        .transpose()?;
    let blocks = exact.map_or(Ok(0..0), |(plan, ordinal)| plan.group_blocks(ordinal))?;
    let count = blocks.end - blocks.start;
    if exact.is_some() && exact_count(&setup, unit.id(), &dc, session)? != count {
        return Err(Error::Corrupt("V4 exact plan/control count disagreement"));
    }
    let target = if count == 0 {
        None
    } else {
        let height = u32::try_from(count.checked_mul(8).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        Some(Plane::empty(
            &ImageInfo::new(8, height, Channels::Rgb, SampleType::U8),
            Rect::new(0, 0, 8, height),
            session,
        )?)
    };
    let mut pending = Pending {
        unit,
        blocks,
        carried: 0..0,
        exact: target,
        copied: 0,
    };
    if let Some((plan, _)) = exact {
        let per = plan.blocks_per_stripe();
        if count != 0 {
            for id in pending.blocks.start / per..pending.blocks.end.div_ceil(per) {
                let source = stripe(id)?;
                if source.role() != 3 || source.id() != id {
                    return Err(Error::Corrupt("V4 addressed exact identity"));
                }
                let source = exact_cache.get(&source, plan.stripe_rect(id)?, session)?;
                Assembler::<C>::copy_exact(&mut pending, plan, id, &source)?;
            }
        }
    }
    if pending.copied != count {
        return Err(Error::Corrupt("V4 addressed exact coverage"));
    }
    Ok(LossyWork {
        unit: pending.unit,
        dc,
        alpha,
        exact: pending.exact.map(Arc::new),
    })
}

struct Pending<'a> {
    unit: LossyUnit<'a>,
    blocks: Range<u64>,
    carried: Range<u64>,
    exact: Option<Plane>,
    copied: u64,
}

/// Pixel dependency state for one forward traversal. A complete AC group is
/// returned as soon as its own dependencies arrive; tile close stays separate.
pub(crate) struct Assembler<'a, 's, C> {
    setup: Arc<LossySetup>,
    tile: Option<SpatialTile>,
    dc: Vec<Option<Arc<Plane>>>,
    _slots: V4Retained,
    alpha: Option<Arc<Plane>>,
    exact: Option<(u64, Arc<Plane>)>,
    plan: Option<Arc<ExactPlan>>,
    ordinal: u64,
    pending: Option<Pending<'a>>,
    session: &'s DecodeSession<C>,
}

#[cfg(feature = "std")]
pub(crate) struct AssemblerState {
    setup: Arc<LossySetup>,
    tile: Option<SpatialTile>,
    dc: Vec<Option<Arc<Plane>>>,
    _slots: V4Retained,
    alpha: Option<Arc<Plane>>,
    exact: Option<(u64, Arc<Plane>)>,
    plan: Option<Arc<ExactPlan>>,
    ordinal: u64,
    pending: Option<Pending<'static>>,
}
#[cfg(feature = "std")]
impl AssemblerState {
    pub(crate) fn bind<'a, 's, C: Fn() -> bool + Sync>(
        self,
        session: &'s DecodeSession<C>,
    ) -> Assembler<'a, 's, C> {
        Assembler {
            setup: self.setup,
            tile: self.tile,
            dc: self.dc,
            _slots: self._slots,
            alpha: self.alpha,
            exact: self.exact,
            plan: self.plan,
            ordinal: self.ordinal,
            pending: self.pending,
            session,
        }
    }
}

impl<'a, 's, C: Fn() -> bool + Sync> Assembler<'a, 's, C> {
    #[cfg(feature = "std")]
    pub(crate) fn into_state(self) -> Result<AssemblerState, Error> {
        let pending = self
            .pending
            .map(|p| {
                Ok::<_, Error>(Pending {
                    unit: p.unit.into_owned(self.session)?,
                    blocks: p.blocks,
                    carried: p.carried,
                    exact: p.exact,
                    copied: p.copied,
                })
            })
            .transpose()?;
        Ok(AssemblerState {
            setup: self.setup,
            tile: self.tile,
            dc: self.dc,
            _slots: self._slots,
            alpha: self.alpha,
            exact: self.exact,
            plan: self.plan,
            ordinal: self.ordinal,
            pending,
        })
    }
    pub(crate) fn new(setup: Arc<LossySetup>, session: &'s DecodeSession<C>) -> Self {
        Self {
            setup,
            tile: None,
            dc: Vec::new(),
            _slots: V4Retained::default(),
            alpha: None,
            exact: None,
            plan: None,
            ordinal: 0,
            pending: None,
            session,
        }
    }

    fn slot(&self, id: u64) -> Result<usize, Error> {
        let tile = self.tile.ok_or(Error::Corrupt("V4 pixels outside tile"))?;
        let column = id % u64::from(self.setup.layout.group_grid().0);
        usize::try_from(
            column
                .checked_sub(u64::from(tile.group_x))
                .filter(|&n| n < u64::from(tile.groups_wide))
                .ok_or(Error::Corrupt("V4 DC cache column"))?,
        )
        .map_err(|_| Error::TooLarge)
    }

    fn exact_count(&self, id: u64) -> Result<u64, Error> {
        let dc = self.dc[self.slot(id)?]
            .as_ref()
            .ok_or(Error::Corrupt("V4 exact control/DC missing"))?;
        exact_count(&self.setup, id, dc, self.session)
    }

    fn copy_exact(
        pending: &mut Pending<'_>,
        plan: &ExactPlan,
        stripe: u64,
        source: &Plane,
    ) -> Result<(), Error> {
        let Some(target) = &mut pending.exact else {
            return Ok(());
        };
        let first = stripe
            .checked_mul(plan.blocks_per_stripe())
            .ok_or(Error::TooLarge)?;
        let a = pending.blocks.start.max(first);
        let b = pending.blocks.end.min(
            first
                .checked_add(plan.blocks_per_stripe())
                .ok_or(Error::TooLarge)?,
        );
        for block in a..b {
            let local = usize::try_from(block - first).map_err(|_| Error::TooLarge)?;
            let dst = usize::try_from(block - pending.blocks.start).map_err(|_| Error::TooLarge)?;
            let packed = source.info.width as usize / 8;
            let x = local % packed * 8;
            let y = local / packed * 8;
            for row in 0..8 {
                target.pixels[(dst * 8 + row) * target.stride..][..24]
                    .copy_from_slice(&source.pixels[(y + row) * source.stride + x * 3..][..24]);
            }
            pending.copied += 1;
        }
        Ok(())
    }

    fn complete(&mut self) -> Result<LossyWork<'a>, Error> {
        let pending = self
            .pending
            .take()
            .ok_or(Error::Corrupt("V4 missing pending AC group"))?;
        if !pending.carried.is_empty()
            || pending.copied != pending.blocks.end - pending.blocks.start
        {
            return Err(Error::Corrupt("incomplete V4 exact dependency"));
        }
        let dc = Arc::clone(
            self.dc[self.slot(pending.unit.id())?]
                .as_ref()
                .ok_or(Error::Corrupt("V4 DC dependency missing"))?,
        );
        self.ordinal += 1;
        Ok(LossyWork {
            unit: pending.unit,
            dc,
            alpha: self.alpha.take(),
            exact: pending.exact.map(Arc::new),
        })
    }

    pub(crate) fn event(&mut self, event: LossyEvent<'a>) -> Result<Option<LossyWork<'a>>, Error> {
        match event {
            LossyEvent::Begin { tile, exact, .. } => {
                self.dc = Vec::new();
                self._slots = V4Retained::default();
                let scope = self.session.v4_model_scope();
                let count = usize::try_from(tile.groups_wide).map_err(|_| Error::TooLarge)?;
                self.session.v4_models().hint(
                    count
                        .checked_mul(core::mem::size_of::<Option<Arc<Plane>>>())
                        .ok_or(Error::TooLarge)?,
                )?;
                self.dc
                    .try_reserve_exact(count)
                    .map_err(|_| Error::TooLarge)?;
                self.dc.resize_with(count, || None);
                self._slots = scope.commit()?;
                self.tile = Some(tile);
                self.plan = exact;
                self.ordinal = 0;
                self.exact = None;
            }
            LossyEvent::Unit { work, .. } => match work.role() {
                0 => {
                    let slot = self.slot(work.id())?;
                    self.dc[slot] = None;
                    let (_, rect) = self.setup.dc_for_group(work.id())?;
                    self.dc[slot] = Some(Arc::new(Plane::decode(&work, rect, self.session)?));
                }
                2 => {
                    self.alpha = Some(Arc::new(Plane::decode(
                        &work,
                        self.setup.group_rect(work.id())?,
                        self.session,
                    )?))
                }
                1 => {
                    let blocks = self
                        .plan
                        .as_ref()
                        .map_or(Ok(0..0), |p| p.group_blocks(self.ordinal))?;
                    let carried = self
                        .plan
                        .as_ref()
                        .map_or(Ok(0..0), |p| p.carried_by(self.ordinal))?;
                    let count = blocks.end - blocks.start;
                    if self.plan.is_some() && self.exact_count(work.id())? != count {
                        return Err(Error::Corrupt("V4 exact plan/control count disagreement"));
                    }
                    let exact = if count == 0 {
                        None
                    } else {
                        let height = u32::try_from(count.checked_mul(8).ok_or(Error::TooLarge)?)
                            .map_err(|_| Error::TooLarge)?;
                        Some(Plane::empty(
                            &ImageInfo::new(8, height, Channels::Rgb, SampleType::U8),
                            Rect::new(0, 0, 8, height),
                            self.session,
                        )?)
                    };
                    let mut pending = Pending {
                        unit: work,
                        blocks,
                        carried,
                        exact,
                        copied: 0,
                    };
                    if let (Some(plan), Some((stripe, pixels))) = (&self.plan, &self.exact) {
                        Self::copy_exact(&mut pending, plan, *stripe, pixels)?;
                    }
                    let ready = pending.carried.is_empty();
                    self.pending = Some(pending);
                    if ready {
                        return self.complete().map(Some);
                    }
                }
                3 => {
                    let plan = self
                        .plan
                        .as_ref()
                        .ok_or(Error::Corrupt("V4 exact stripe without plan"))?;
                    let pending = self
                        .pending
                        .as_mut()
                        .ok_or(Error::Corrupt("V4 exact stripe without AC group"))?;
                    if pending.carried.start != work.id() {
                        return Err(Error::Corrupt("V4 exact ownership order"));
                    }
                    self.exact = None;
                    let pixels = Arc::new(Plane::decode(
                        &work,
                        plan.stripe_rect(work.id())?,
                        self.session,
                    )?);
                    Self::copy_exact(pending, plan, work.id(), &pixels)?;
                    pending.carried.start += 1;
                    let ready = pending.carried.is_empty();
                    self.exact = Some((work.id(), pixels));
                    if ready {
                        return self.complete().map(Some);
                    }
                }
                _ => return Err(Error::Corrupt("V4 lossy plane role")),
            },
            LossyEvent::End { .. } => {
                if self.pending.is_some() {
                    return Err(Error::Corrupt("V4 pending pixels at tile close"));
                }
                self.dc = Vec::new();
                self._slots = V4Retained::default();
                self.alpha = None;
                self.exact = None;
                self.tile = None;
                self.plan = None;
            }
            _ => {}
        }
        Ok(None)
    }
}
