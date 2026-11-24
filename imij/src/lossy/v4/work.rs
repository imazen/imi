//! Independent V4 unit work. Tables are immutable; inputs/output are caller-owned.

use alloc::{borrow::Cow, sync::Arc, vec::Vec};
use bytemuck::allocation::try_zeroed_vec;

use super::super::*;
use super::LossySetup;
use super::models::{LossyModels, Model};
use crate::decode_resources::DecodeSession;
#[cfg(feature = "std")]
use crate::parse::limits::Admission;
use crate::planes::Rect;
use crate::{ByteReader, ImageRef};

/// Decoded dependencies of one AC group. DC and alpha cover this group's
/// rectangle, not their possibly larger stored coverage. Exact pixels are
/// full 8×8 blocks stacked vertically in parent-raster selection order (RGB8).
/// A reference is needed only when the group's control codes actually keep
/// pixels. Every view supports strided rows.
#[derive(Clone, Copy)]
pub struct LossyGroupInputs<'a> {
    pub dc: ImageRef<'a>,
    pub alpha: Option<ImageRef<'a>>,
    pub exact: Option<ImageRef<'a>>,
    pub reference: Option<ImageRef<'a>>,
}

/// One unit with a captured immutable model. It can outlive model selection
/// and run on a different worker; no preceding unit's entropy/predictor state
/// is needed. The caller owns input/dependency/output queue budgets.
pub struct LossyUnit<'a> {
    setup: Arc<LossySetup>,
    model: Model,
    role: u8,
    id: u64,
    settings: Cow<'a, [u8]>,
    entropy: Cow<'a, [u8]>,
    _owned: Option<crate::decode_resources::V4Retained>,
    compact: bool,
}

pub(crate) struct StoredLossyUnit {
    setup: Arc<LossySetup>,
    model: Model,
    role: u8,
    id: u64,
    settings: core::ops::Range<usize>,
    entropy: core::ops::Range<usize>,
    compact: bool,
}
impl StoredLossyUnit {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn rect(&self) -> Result<Rect, Error> {
        self.setup.group_rect(self.id)
    }
    pub(crate) fn bind<'a>(&self, input: &'a [u8]) -> Result<LossyUnit<'a>, Error> {
        Ok(LossyUnit {
            setup: Arc::clone(&self.setup),
            model: self.model.clone(),
            role: self.role,
            id: self.id,
            compact: self.compact,
            settings: input
                .get(self.settings.clone())
                .ok_or(Error::Corrupt("V4 stored settings span"))?
                .into(),
            entropy: input
                .get(self.entropy.clone())
                .ok_or(Error::Corrupt("V4 stored entropy span"))?
                .into(),
            _owned: None,
        })
    }
}
impl<C: Fn() -> bool + Sync> LossyModels<'_, C> {
    /// Read one main object's body after its role was established by the
    /// record kind. Object framing/integrity and physical schedule belong to
    /// the owning driver. Model lifetimes are consumed by this operation.
    pub fn unit<'a>(&mut self, role: u8, bytes: &'a [u8]) -> Result<LossyUnit<'a>, Error> {
        read_unit(self.shared_setup(), role, bytes, self)
    }
}

pub(crate) fn read_unit<'a>(
    setup: Arc<LossySetup>,
    role: u8,
    bytes: &'a [u8],
    resolver: &mut dyn super::models::ModelResolver,
) -> Result<LossyUnit<'a>, Error> {
    let mut r = ByteReader::new(bytes);
    crate::trace::trace_enter_addr(
        "payload",
        "V4:4",
        match role {
            0 => "DC coverage",
            1 => "AC group",
            2 => "alpha group",
            _ => "packed exact stripe",
        },
        r.mark(),
    );
    let id = crate::trace::tf!("unit ID", r, r.varint()?);
    let settings = if role == 1 {
        let length = crate::trace::tf!("settings bytes", r, r.var_usize()?);
        r.bytes(length)?
    } else {
        &[]
    };
    let model_id = crate::trace::tf!("model ID", r, r.varint()?);
    let compact = if role == 1 && setup.as_ref().coder == LossyEntropy::RunLevel {
        match crate::trace::tf!("compact restart", r, r.varint()?) {
            0 => false,
            1 => true,
            _ => return Err(Error::Corrupt("V4 AC restart form")),
        }
    } else {
        false
    };
    let model = resolver.take(role, id, model_id)?;
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() && role == 1 {
        resolver.trace_settings(&setup, id, settings)?;
    }

    crate::trace::tnote!(
        "model dependency",
        alloc::format!("role {role}, model {model_id}")
    );
    if crate::trace::on() && role != 3 {
        let rect = if role == 0 {
            setup.as_ref().dc_for_group(id)?.1
        } else {
            setup.as_ref().group_rect(id)?
        };
        let scale = if role == 0 { 8u64 } else { 1 };
        let x = u64::from(rect.x()) * scale;
        let y = u64::from(rect.y()) * scale;
        crate::trace::region(
            x,
            y,
            (u64::from(rect.width()) * scale).min(u64::from(setup.as_ref().info().width) - x),
            (u64::from(rect.height()) * scale).min(u64::from(setup.as_ref().info().height) - y),
        );
    }
    crate::trace::exit_addr(crate::trace::Mark::Addr(bytes.as_ptr() as usize).plus(bytes.len()));

    Ok(LossyUnit {
        setup,
        model,
        role,
        id,
        settings: settings.into(),
        entropy: (&bytes[r.pos..]).into(),
        _owned: None,
        compact,
    })
}

fn extent(
    width: usize,
    rows: usize,
    stride: usize,
    bpp: usize,
    length: usize,
) -> Result<usize, Error> {
    let row = width.checked_mul(bpp).ok_or(Error::TooLarge)?;
    let required = rows
        .checked_sub(1)
        .and_then(|n| n.checked_mul(stride))
        .and_then(|n| n.checked_add(row))
        .ok_or(Error::TooLarge)?;
    if stride < row || length < required {
        return Err(Error::InvalidInput("V4 pixel stride/extent"));
    }
    Ok(row)
}

fn view(input: ImageRef<'_>, expected: &ImageInfo, width: u32, height: u32) -> Result<(), Error> {
    let mut info = expected.clone();
    info.width = width;
    info.height = height;
    if input.info != &info {
        return Err(Error::InvalidInput("V4 dependency pixel interpretation"));
    }
    extent(
        width as usize,
        height as usize,
        input.stride,
        info.channels.count() * info.sample.bytes(),
        input.pixels.len(),
    )?;
    Ok(())
}

fn reserved<T: Clone>(count: usize, value: T) -> Result<Vec<T>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
    out.resize(count, value);
    Ok(out)
}

impl LossyUnit<'_> {
    #[cfg(all(feature = "lab", feature = "encode"))]
    pub(crate) fn inspect_coefficients<C: Fn() -> bool + Sync>(
        &self,
        output: &mut [i32],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<usize, Error> {
        let Model::Ac(model) = &self.model else {
            return Err(Error::InvalidInput("numeric unit is not AC"));
        };
        let rect = self.setup.group_rect(self.id)?;
        self.setup.settings.visit_unit(
            &self.settings,
            rect.height().div_ceil(8),
            rect.width().div_ceil(8),
            self.setup.declared,
            session,
            |_, _, state| {
                if state.partition() != 0 || state.plane_plans() != [0; 12] {
                    return Err(Error::Unsupported(
                        "lab coefficient export requires plain 8x8 plans",
                    ));
                }
                Ok(())
            },
        )?;
        model.decode_into(
            rect.width(),
            rect.height(),
            self.compact,
            self.ac_entropy()?.0,
            output,
            stride,
            session,
        )?;
        Ok(self.entropy.len())
    }

    #[cfg(all(feature = "lab", feature = "encode"))]
    pub(crate) fn inspection_entropy_bytes(&self) -> usize {
        self.entropy.len()
    }
    fn settings_work(&self) -> u128 {
        let rect = self
            .setup
            .group_rect(self.id)
            .expect("admitted group identity");
        4 * (u128::from(rect.width().div_ceil(16)) * u128::from(rect.height().div_ceil(16))
            + self.settings.len() as u128
            + 1)
    }
    fn map_budget(&self, rect: Rect) -> (u128, u128) {
        crate::v4::map_budget(
            &self.settings,
            rect.height().div_ceil(8),
            rect.width().div_ceil(8),
        )
    }
    pub(crate) fn numeric_execution_cost(&self, rect: Rect) -> Result<(u128, u128), Error> {
        let Model::Numeric(model) = &self.model else {
            return Err(Error::InvalidInput("AC unit is not numeric"));
        };
        model.execution_cost(rect)
    }

    pub(crate) fn ac_execution_cost(&self) -> Result<(u128, u128), Error> {
        let rect = self.setup.group_rect(self.id)?;
        let width = u128::from(rect.width());
        let height = u128::from(rect.height());
        let bw = width.div_ceil(8);
        let bh = height.div_ceil(8);
        let blocks = bw * bh;
        let bps = self.setup.info.sample.bytes() as u128;
        let rgb = width * height * 3 * bps;
        let (map_bytes, map_work) = self.map_budget(rect);
        let scratch = crate::decoder_memory::lossy_stripe_bytes(width, bh)?
            .checked_mul(2)
            .and_then(|n| {
                n.checked_add(
                    rgb + blocks * 64
                        + 64 * 1024
                        + decoder_recon_bytes()
                        + height * core::mem::size_of::<&mut [u8]>() as u128
                        + map_bytes,
                )
            })
            .ok_or(Error::TooLarge)?;
        let dc = &self.setup.numeric[0]
            .as_ref()
            .ok_or(Error::Corrupt("V4 DC geometry"))?
            .info;
        let work = blocks * (NCH * 64) as u128
            + self.entropy.len() as u128
            + rgb
            + bw * bh * dc.bytes_per_pixel() as u128
            + width * height * self.setup.info.bytes_per_pixel() as u128
            + self.settings_work()
            + map_work;
        Ok((scratch, work))
    }
    /// Only a group waiting for later exact stripes needs a private copy.
    /// Ordinary borrowed whole-file and immediately ready jobs stay borrowed.
    #[cfg(feature = "std")]
    pub(crate) fn into_owned<C: Fn() -> bool + Sync>(
        self,
        session: &DecodeSession<C>,
    ) -> Result<LossyUnit<'static>, Error> {
        let scope = session.v4_model_scope();
        let copy = |bytes: Cow<'_, [u8]>| -> Result<Vec<u8>, Error> {
            match bytes {
                Cow::Owned(bytes) => Ok(bytes),
                Cow::Borrowed(bytes) => {
                    session.v4_models().hint(bytes.len())?;
                    session.adapter_work(bytes.len() as u128)?;
                    let mut copy = Vec::new();
                    copy.try_reserve_exact(bytes.len())
                        .map_err(|_| Error::TooLarge)?;
                    copy.extend_from_slice(bytes);
                    Ok(copy)
                }
            }
        };
        let settings = Cow::Owned(copy(self.settings)?);
        let entropy = Cow::Owned(copy(self.entropy)?);
        // An already-owned pending unit keeps its original charge. Its byte
        // buffers move without another copy or reservation on subsequent steps.
        let charge = scope.commit()?;
        Ok(LossyUnit {
            setup: self.setup,
            model: self.model,
            role: self.role,
            id: self.id,
            settings,
            entropy,
            compact: self.compact,
            _owned: self._owned.or(Some(charge)),
        })
    }
    pub(crate) fn store(self, input: &[u8]) -> Result<StoredLossyUnit, Error> {
        Ok(StoredLossyUnit {
            settings: crate::owned::span(input, &self.settings)?,
            entropy: crate::owned::span(input, &self.entropy)?,
            setup: self.setup,
            model: self.model,
            role: self.role,
            id: self.id,
            compact: self.compact,
        })
    }
    pub(super) fn shared_setup(&self) -> Arc<LossySetup> {
        Arc::clone(&self.setup)
    }
    pub const fn id(&self) -> u64 {
        self.id
    }
    pub const fn role(&self) -> u8 {
        self.role
    }
    pub fn setup(&self) -> &LossySetup {
        &self.setup
    }

    /// Decode a DC/alpha/exact rectangle. DC/alpha identities are checked here;
    /// exact packing also requires the owning tile's admitted begin plan.
    pub fn decode_numeric_into<C: Fn() -> bool + Sync>(
        &self,
        rect: Rect,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        let Model::Numeric(model) = &self.model else {
            return Err(Error::InvalidInput("AC unit is not numeric"));
        };
        let expected = match self.role {
            0 => {
                let (id, expected) = self.setup.dc_for_group(self.id)?;
                if id != self.id {
                    return Err(Error::Corrupt("V4 DC unit is not span start"));
                }
                expected
            }
            2 => self.setup.group_rect(self.id)?,
            3 => {
                let (_, rows) = self
                    .setup
                    .exact
                    .ok_or(Error::Corrupt("V4 exact descriptor missing"))?;
                if u64::from(rect.y())
                    != self
                        .id
                        .checked_mul(u64::from(rows))
                        .ok_or(Error::TooLarge)?
                    || rect.x() != 0
                    || rect.width() != model.info().width
                    || rect.height() > rows
                {
                    return Err(Error::Corrupt("V4 packed exact stripe identity"));
                }
                rect
            }
            _ => return Err(Error::Corrupt("V4 numeric role")),
        };
        if rect != expected {
            return Err(Error::Corrupt("V4 numeric coverage rectangle"));
        }
        model.decode_rectangle(rect, &self.entropy, output, stride, session)
    }

    /// Reconstruct one AC group using already decoded numeric dependencies.
    /// Settings/control/unused-coefficient checks precede output publication.
    pub fn decode_into<C: Fn() -> bool + Sync>(
        &self,
        input: LossyGroupInputs<'_>,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<Rect, Error> {
        let (entropy, expected) = self.ac_entropy()?;
        let rect = self.decode_ac(input, output, stride, session, entropy)?;
        if let Some(checks) = &self.setup.checks {
            let mut info = self.setup.info.clone();
            info.width = rect.width();
            info.height = rect.height();
            checks.verify_pixels(
                crate::v4::CheckScope::GroupPixels,
                crate::v4::PixelCheckIdentity {
                    image: 0,
                    id: self.id,
                    x: rect.x(),
                    y: rect.y(),
                },
                ImageRef::new(output, stride, &info),
                expected,
                session,
            )?;
        }
        Ok(rect)
    }

    fn ac_entropy(&self) -> Result<(&[u8], &[u8]), Error> {
        self.setup
            .checks
            .as_ref()
            .map_or(Ok((&self.entropy[..], &[][..])), |c| {
                c.split(crate::v4::CheckScope::GroupPixels, &self.entropy)
            })
    }

    fn decode_ac<C: Fn() -> bool + Sync>(
        &self,
        input: LossyGroupInputs<'_>,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
        entropy: &[u8],
    ) -> Result<Rect, Error> {
        let Model::Ac(model) = &self.model else {
            return Err(Error::InvalidInput("numeric unit is not AC"));
        };
        let p = &self.setup;
        let rect = p.group_rect(self.id)?;
        let (width, height) = (rect.width() as usize, rect.height() as usize);
        let (bw, bh) = (width.div_ceil(8), height.div_ceil(8));
        let blocks = bw.checked_mul(bh).ok_or(Error::TooLarge)?;
        let coefficients = blocks.checked_mul(NCH * 64).ok_or(Error::TooLarge)?;
        let bps = p.info.sample.bytes();
        let row = extent(
            width,
            height,
            stride,
            p.info.channels.count() * bps,
            output.len(),
        )?;
        let mut local = p.info.clone();
        local.width = rect.width();
        local.height = rect.height();
        session.v4_output(&local)?;
        let dc_info = &p.numeric[0].as_ref().unwrap().info;
        view(input.dc, dc_info, bw as u32, bh as u32)?;
        if p.info.channels.has_alpha() != input.alpha.is_some() {
            return Err(Error::InvalidInput("V4 group alpha dependency"));
        }
        if let Some(alpha) = input.alpha {
            view(
                alpha,
                &p.numeric[2].as_ref().unwrap().info,
                rect.width(),
                rect.height(),
            )?;
        }
        if let Some(reference) = input.reference {
            view(reference, &p.info, rect.width(), rect.height())?;
        }
        let rgb_bytes = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(3 * bps))
            .ok_or(Error::TooLarge)?;
        let (scratch, work) = self.ac_execution_cost()?;
        let (map_bytes, map_work) = self.map_budget(rect);
        let _working = session.v4_working(scratch - map_bytes)?;
        session.adapter_work(work - self.settings_work() - map_work)?;
        let g = Geom::new(rect.width(), rect.height(), bh as u32);
        let q = try_zeroed_vec(coefficients.checked_add(64).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        let mut eobs = Vec::new();
        eobs.try_reserve_exact(
            blocks
                .checked_mul(NCH)
                .and_then(|n| n.checked_add(1))
                .ok_or(Error::TooLarge)?,
        )
        .map_err(|_| Error::TooLarge)?;
        let (mut q, eobs) = model.decode_quantized(&g, self.compact, entropy, q, eobs)?;
        let mut dc = Vec::new();
        let mut scales = Vec::new();
        dc.try_reserve_exact(blocks.checked_mul(3).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        scales
            .try_reserve_exact(blocks)
            .map_err(|_| Error::TooLarge)?;
        let dc_row = bw * dc_info.channels.count() * dc_info.sample.bytes();
        for y in 0..bh {
            session.adapter_work(0)?;
            dc_values(
                dc_info.sample,
                p.control != 0,
                p.control == 2,
                &input.dc.pixels[y * input.dc.stride..][..dc_row],
                &mut dc,
                &mut scales,
            )?;
        }
        let skips = matches!(p.control, 2..=4);
        let exact_count = if p.control == 2 {
            scales.iter().filter(|&&a| a == 0.0).count()
        } else {
            0
        };
        if exact_count != 0 {
            let exact = input
                .exact
                .ok_or(Error::InvalidInput("V4 exact pixels missing"))?;
            let exact_info = ImageInfo::new(
                8,
                u32::try_from(exact_count.checked_mul(8).ok_or(Error::TooLarge)?)
                    .map_err(|_| Error::TooLarge)?,
                Channels::Rgb,
                SampleType::U8,
            );
            view(exact, &exact_info, exact_info.width, exact_info.height)?;
        }
        if matches!(p.control, 3 | 4) && scales.contains(&0.0) && input.reference.is_none() {
            return Err(Error::Corrupt("V4 kept pixels require a reference"));
        }
        fill_dc(&mut q, &dc, bh, bw);
        let mut planes = reserved(2 * NCH * 8 * 32, 0f32)?;
        let mut linear = reserved(16 * 3, 0f32)?;
        let render = Reconstruction {
            info: &p.info,
            color: p.color,
            xyb_power: p.parameters.xyb_power,
            inv_scale: p.parameters.scale.map(|s| 1.0 / s),
            dq: &p.parameters.dq,
            bias: &p.parameters.bias,
            transform: &p.parameters.transform,
            layouts: [LossyChannelLayout::default(); 3],
            layout_steps: &EMPTY_LAYOUT_STEPS,
            xfer16: &p.parameters.xfer16,
            code0_skips: skips,
            clear_q: false,
            mode: p.coder,
        };
        let kind = InputKind::of(&p.info).unwrap();
        let settings =
            p.settings
                .prepare_unit(&self.settings, (bh as u32, bw as u32), p.declared, session)?;
        let mut has_merge = false;
        let mut has_small = false;
        let mut has_subblock = false;
        let mut roles = Vec::new();
        let mut small_split = Vec::new();
        let mut borrowed_small: &[u8] = &[];
        if let Some((partitions, flags)) = settings.joint_maps() {
            has_merge = expand_partitions(partitions, bh, bw, skips, &scales, &mut roles)?;
            if flags.iter().any(|&f| f != 0) {
                has_small = true;
                borrowed_small = flags;
            }
        } else if let Some(partitions) = settings.range_partitions() {
            has_merge = expand_partitions(partitions, bh, bw, skips, &scales, &mut roles)?;
        } else if !settings.inherits_default() {
            settings.visit(|cy, cx, state| {
                let (x, y) = (cx as usize * 2, cy as usize * 2);
                let (cw, ch) = (2.min(bw - x), 2.min(bh - y));
                let part = state.partition();
                let plans = state.plane_plans();
                let codes = state.ac_codes();
                if scales.is_empty() && codes.iter().any(|&v| v != 0) {
                    scales
                        .try_reserve_exact(blocks)
                        .map_err(|_| Error::TooLarge)?;
                    scales.resize(blocks, 1.0);
                }
                if part != 0 && !has_merge {
                    has_merge = true;
                    roles = reserved(blocks, 0u8)?;
                }
                for yy in 0..ch {
                    for xx in 0..cw {
                        let j = yy * 2 + xx;
                        let at = (y + yy) * bw + x + xx;
                        let bypass = skips && scales.get(at).copied() == Some(0.0);
                        let code = codes[j];
                        if code != 0 && !bypass {
                            scales[at] = f32::from(code) / 64.0;
                        }
                        if part != 0 {
                            let role = superblock::PARTITIONS[part as usize][j];
                            if role != 0 && bypass {
                                return Err(Error::Corrupt("V4 merge with bypassed parent"));
                            }
                            roles[at] = role;
                        }
                        for color in 0..NCH {
                            let plan = plans[j * 3 + color];
                            if plan == 1 {
                                if !has_small {
                                    has_small = true;
                                    small_split = reserved(blocks, 0u8)?;
                                }
                                small_split[at] |= 1 << color;
                            } else if plan >= 2 {
                                has_subblock = true;
                            }
                        }
                    }
                }
                Ok(())
            })?;
        }
        if has_merge {
            superblock::validate(&roles, &scales, bw, skips)?;
        }
        let merged_tables = if has_merge {
            Some(
                p.parameters
                    .merged
                    .as_deref()
                    .ok_or(Error::Corrupt("V4 merge tables missing"))?,
            )
        } else {
            None
        };
        let small = if has_small {
            simd::Small {
                split: if borrowed_small.is_empty() {
                    &small_split
                } else {
                    borrowed_small
                },
                dq: p
                    .parameters
                    .small
                    .as_deref()
                    .ok_or(Error::Corrupt("V4 small tables missing"))?,
                int: false,
            }
        } else {
            simd::Small::NONE
        };
        let rgb_row = width * 3 * bps;
        // Direct RGB reconstruction writes every pixel and needs no staging copy
        // for ordinary 8x8, superblock merges, and 4x4 small transforms.
        let mut staged = if !has_subblock && !skips && p.info.channels == Channels::Rgb {
            None
        } else {
            Some(try_zeroed_vec(rgb_bytes).map_err(|_| Error::TooLarge)?)
        };
        let (rgb, rgb_stride) = match &mut staged {
            Some(rgb) => (rgb.as_mut_slice(), rgb_row),
            None => (&mut *output, stride),
        };
        if !has_subblock {
            let mut rows = Vec::new();
            rows.try_reserve_exact(height)
                .map_err(|_| Error::TooLarge)?;
            rows.extend(
                rgb.chunks_mut(rgb_stride)
                    .take(height)
                    .map(|row| &mut row[..rgb_row]),
            );
            if let Some(tables) = merged_tables {
                superblock::recon_stripe::<true>(
                    &render,
                    tables,
                    &g,
                    0,
                    &scales,
                    &roles,
                    &mut rows,
                    true,
                    &mut q,
                    &eobs,
                    &mut planes,
                    &mut linear,
                    small,
                );
            } else {
                recon_stripe_mode::<true>(
                    &render,
                    &g,
                    0,
                    &scales,
                    layout::ModeMap::Constant(0),
                    &mut rows,
                    true,
                    &mut q,
                    &eobs,
                    &mut planes,
                    &mut linear,
                    small,
                );
            }
        } else {
            session.adapter_work(self.settings_work() / 2)?;
            decode_ac_subblocks(
                p,
                &render,
                &settings,
                &q,
                &eobs,
                &scales,
                &mut planes,
                &mut linear,
                rgb,
                rgb_stride,
                (width, height, bw, bh, bps),
                skips,
                kind,
            )?;
        }
        let Some(mut rgb) = staged else {
            session.adapter_work(0)?;
            return Ok(rect);
        };
        let mut exact_at = 0usize;
        for by in 0..bh {
            for bx in 0..bw {
                if !skips || scales[by * bw + bx] != 0.0 {
                    continue;
                }
                let (source, sx, sy) = if p.control == 2 {
                    let source = input.exact.unwrap();
                    let y = exact_at * 8;
                    exact_at += 1;
                    (source, 0, y)
                } else {
                    (input.reference.unwrap(), bx * 8, by * 8)
                };
                let bytes = (8.min(width - bx * 8)) * 3 * bps;
                for y in 0..8.min(height - by * 8) {
                    let dst = ((by * 8 + y) * width + bx * 8) * 3 * bps;
                    rgb[dst..][..bytes].copy_from_slice(
                        &source.pixels[(sy + y) * source.stride + sx * 3 * bps..][..bytes],
                    );
                }
            }
        }
        for y in 0..height {
            session.adapter_work(0)?;
            let alpha = input
                .alpha
                .map(|a| &a.pixels[y * a.stride..][..width * bps]);
            pack_rows(
                &p.info,
                width,
                &rgb[y * width * 3 * bps..][..width * 3 * bps],
                alpha,
                &mut [&mut output[y * stride..][..row]],
            );
        }
        Ok(rect)
    }
}

const EMPTY_LAYOUT_STEPS: [[[f32; 64]; 2]; 3] = [[[0f32; 64]; 2]; 3];

#[inline]
fn expand_partitions(
    partitions: &[u8],
    bh: usize,
    bw: usize,
    skips: bool,
    scales: &[f32],
    roles: &mut Vec<u8>,
) -> Result<bool, Error> {
    if !partitions.iter().any(|&part| part != 0) {
        return Ok(false);
    }
    let blocks = bh.checked_mul(bw).ok_or(Error::TooLarge)?;
    *roles = reserved(blocks, 0u8)?;
    let cols = bw.div_ceil(2);
    for (i, &part) in partitions.iter().enumerate() {
        if part == 0 {
            continue;
        }
        let y = (i / cols) * 2;
        let x = (i % cols) * 2;
        let ch = 2.min(bh - y);
        let cw = 2.min(bw - x);
        let pat = superblock::PARTITIONS[part as usize];
        for yy in 0..ch {
            for xx in 0..cw {
                let role = pat[yy * 2 + xx];
                if role != 0 {
                    let at = (y + yy) * bw + x + xx;
                    if skips && scales.get(at).copied() == Some(0.0) {
                        return Err(Error::Corrupt("V4 merge with bypassed parent"));
                    }
                    roles[at] = role;
                }
            }
        }
    }
    Ok(true)
}

#[cold]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn decode_ac_subblocks<C: Fn() -> bool + Sync>(
    p: &LossySetup,
    render: &Reconstruction<'_>,
    settings: &crate::v4::PreparedUnit<'_, '_, C>,
    q: &[i32],
    eobs: &[u8],
    scales: &[f32],
    planes: &mut [f32],
    linear: &mut [f32],
    rgb: &mut [u8],
    rgb_stride: usize,
    (width, height, bw, bh, bps): (usize, usize, usize, usize, usize),
    skips: bool,
    kind: InputKind,
) -> Result<(), Error> {
    settings.visit(|cy, cx, state| {
        let (x, y) = (cx as usize * 2, cy as usize * 2);
        let (cw, ch) = (2.min(bw - x), 2.min(bh - y));
        let mut cell_q = [0i32; 4 * NCH * 64 + 64];
        let mut cell_eobs = [0u8; 4 * NCH];
        let mut cell_scales = [1f32; 4];
        let mut roles = [0u8; 4];
        let codes = state.ac_codes();
        let plans = state.plane_plans();
        for yy in 0..ch {
            for xx in 0..cw {
                let j = yy * 2 + xx;
                let dst = yy * cw + xx;
                let src = (y + yy) * bw + x + xx;
                cell_q[dst * NCH * 64..][..NCH * 64]
                    .copy_from_slice(&q[src * NCH * 64..][..NCH * 64]);
                cell_eobs[dst * NCH..][..NCH].copy_from_slice(&eobs[src * NCH..][..NCH]);
                let base = scales.get(src).copied().unwrap_or(1.0);
                let bypass = skips && base == 0.0;
                cell_scales[dst] = if bypass || codes[j] == 0 {
                    base
                } else {
                    f32::from(codes[j]) / 64.0
                };
                roles[dst] = superblock::PARTITIONS[state.partition() as usize][j];
                if roles[dst] != 0 && bypass {
                    return Err(Error::Corrupt("V4 merge with bypassed parent"));
                }
            }
        }
        if state.partition() != 0 {
            superblock::validate(&roles[..cw * ch], &cell_scales[..cw * ch], cw, skips)?;
        }
        planes.fill(0.0);
        let ps = 32;
        let band = NCH * 8 * ps;
        let mut visited = 0u8;
        for yy in 0..ch {
            for xx in 0..cw {
                let j = yy * 2 + xx;
                let b = yy * cw + xx;
                if visited & (1 << j) != 0 {
                    continue;
                }
                if let Some(shape) = superblock::Shape::from_role(roles[b]) {
                    let tables = p
                        .parameters
                        .merged
                        .as_deref()
                        .ok_or(Error::Corrupt("V4 merge tables missing"))?;
                    let (top, bot) = planes.split_at_mut(band);
                    let mut dst = if yy == 0 {
                        superblock::kernel::Bands {
                            top,
                            bot,
                            stride: ps,
                            off: xx * 8,
                        }
                    } else {
                        superblock::kernel::Bands {
                            top: bot,
                            bot: &mut [],
                            stride: ps,
                            off: xx * 8,
                        }
                    };
                    superblock::recon_merged(
                        tables,
                        p.coder,
                        shape,
                        &mut cell_q,
                        &cell_eobs,
                        (yy, xx, cw),
                        cell_scales[b],
                        &mut dst,
                        true,
                    );
                    for dy in 0..shape.ph() {
                        for dx in 0..shape.pw() {
                            visited |= 1 << ((yy + dy) * 2 + xx + dx);
                        }
                    }
                    continue;
                }
                if skips && cell_scales[b] == 0.0 {
                    continue;
                }
                for color in 0..NCH {
                    let spectrum = &cell_q[(b * NCH + color) * 64..][..64];
                    let plan = plans[j * 3 + color];
                    let pixels = inverse_plane(p, color, plan, spectrum, cell_scales[b])?;
                    for py in 0..8 {
                        planes[yy * band + (color * 8 + py) * ps + xx * 8..][..8]
                            .copy_from_slice(&pixels[py * 8..][..8]);
                    }
                }
            }
        }
        let n = (cw * 8).min(width - x * 8);
        let rows = (ch * 8).min(height - y * 8);
        for py in 0..rows {
            let band = py / 8 * band;
            let sy = py % 8;
            let pl = |c: usize| &planes[band + (c * 8 + sy) * ps..];
            let start = (y * 8 + py) * rgb_stride + x * 8 * 3 * bps;
            color_chunk_row(
                render,
                kind,
                kind == InputKind::U16,
                p.parameters.xfer16.as_ref(),
                true,
                pl(0),
                pl(1),
                pl(2),
                linear,
                &mut rgb[start..][..n * 3 * bps],
                n,
            );
        }
        Ok(())
    })
}

fn inverse_plane(
    p: &LossySetup,
    color: usize,
    plan: u8,
    q: &[i32],
    aq: f32,
) -> Result<[f32; 64], Error> {
    let mut pixels = [0f32; 64];
    if plan == 1 {
        let small = p
            .parameters
            .small
            .as_deref()
            .ok_or(Error::Corrupt("V4 small tables missing"))?;
        let dq = aq_dequant(&small[color], Some(aq));
        for i in 0..64 {
            pixels[i] = q[i] as f32 * dq[i];
        }
        small::inverse_ref(&mut pixels);
    } else if plan == 0 {
        let dq = aq_dequant(&p.parameters.dq[color], Some(aq));
        for i in 0..64 {
            pixels[i] = if let Some(bias) = &p.parameters.bias {
                let sign = q[i].clamp(-1, 1);
                (q[i] as f32 - sign as f32 * bias[color][(q[i] != sign) as usize][i]) * dq[i]
            } else {
                q[i] as f32 * dq[i]
            };
        }
        dct::idct8x8(&mut pixels);
    } else {
        let layout = subblock::Layout::from_byte(plan - 2)?;
        if (layout.samples()..64).any(|k| q[ZIGZAG[k] as usize] != 0) {
            return Err(Error::Corrupt("nonzero unused V4 sampled coefficient"));
        }
        let steps = layout.steps(&p.parameters.steps[color]);
        for i in 0..64 {
            let value = if let Some(bias) = &p.parameters.bias {
                let sign = q[i].clamp(-1, 1);
                q[i] as f32 - sign as f32 * bias[color][(q[i] != sign) as usize][i]
            } else {
                q[i] as f32
            };
            pixels[i] = value * (steps[i] * if i == 0 { 1.0 } else { aq });
        }
        pixels = layout.inverse(&pixels);
    }
    Ok(pixels)
}
