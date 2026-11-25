//! Numeric model definitions share the admitted V2/V3 reconstruction kernels.

use alloc::vec::Vec;

use crate::decode_resources::DecodeSession;
use crate::parse::header::Features;
use crate::{ByteReader, Error, ImageInfo, put_varint};

/// Geometry/interpretation derived from V4 setup, never from a model definition.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NumericGeometry {
    pub info: ImageInfo,
    pub lanes: u8,
    pub stripe_rows: u32,
    /// Zero means full width; otherwise positive and below the plane width.
    pub group_columns: u32,
}

impl NumericGeometry {
    pub fn new(
        info: ImageInfo,
        lanes: u8,
        stripe_rows: u32,
        group_columns: u32,
    ) -> Result<Self, Error> {
        if info.width == 0
            || info.height == 0
            || !(1..=16).contains(&lanes)
            || stripe_rows == 0
            || group_columns >= info.width
        {
            return Err(Error::Corrupt("V4 numeric geometry"));
        }
        info.validate_precision()?;
        Ok(Self {
            info,
            lanes,
            stripe_rows,
            group_columns,
        })
    }
}

/// An immutable V4 numeric definition with checked maps/tables. It decodes
/// numeric DC, alpha or exact-plane groups using existing sample arithmetic.
///
/// This is not a file decoder. The owning V4 driver supplies the interpretation,
/// geometry, admitted bank and definition lifetime, verifies framing/integrity,
/// and assembles the decoded plane into image reconstruction. Serialized model
/// input must already have been charged by that driver. Retained model capacities
/// are charged here; temporary group scratch is released after each call.
pub struct NumericModel {
    parsed: crate::v1::Parsed<'static>,
    output_map: crate::v1::OutMap,
    checks: Option<alloc::sync::Arc<super::OwnedChecks>>,
    _charge: crate::decode_resources::V4Retained,
}

impl NumericModel {
    pub(super) fn read_root<C: Fn() -> bool + Sync>(
        info: ImageInfo,
        head: &[u8],
        declared: Features,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        use crate::parse::limits::Admission;
        session
            .v4_models()
            .hint(core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>())?;
        if ByteReader::new(head).varint()? & crate::v1::H_LAB_BANK != 0 {
            return Err(Error::Unsupported("V4 standalone model bank"));
        }
        let mut input = ByteReader::new(head);
        let parsed =
            crate::v1::parse_head_checked(info, &mut input, &session.v4_models(), declared)?;
        if let Some(profile) = session.v4_profile() {
            profile.check_image(&parsed.info, crate::Coding::Lossless)?;
            profile.check_numeric(&parsed)?;
        }
        let checks = super::OwnedChecks::read_tail(&head[input.pos..], session)?;
        if parsed.hint.is_some() || parsed.reserve.is_some() || parsed.unit_ordinals {
            return Err(Error::Corrupt("legacy numeric index record in V4 setup"));
        }
        let output_map = crate::v1::OutMap::new(&parsed.info, &parsed.layout);
        Ok(Self {
            parsed,
            output_map,
            checks,
            _charge: scope.commit()?,
        })
    }

    pub(super) fn geometry(&self) -> Result<NumericGeometry, Error> {
        NumericGeometry::new(
            self.parsed.info.clone(),
            self.parsed.lanes as u8,
            u32::try_from(self.parsed.hs).map_err(|_| Error::TooLarge)?,
            u32::try_from(self.parsed.gcols).map_err(|_| Error::TooLarge)?,
        )
    }
    /// Parse the geometry-free numeric definition of SPEC_V4 section 4.
    /// `bank_id` is checked against the setup declaration by the outer driver;
    /// this operation also refuses any bank unknown to the implementation.
    pub fn read<C: Fn() -> bool + Sync>(
        geometry: NumericGeometry,
        definition: &[u8],
        bank_id: u64,
        declared: Features,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        use crate::parse::limits::Admission;
        session
            .v4_models()
            .hint(core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>())?;
        // Revalidate public fields, which a caller can edit after construction.
        let geometry = NumericGeometry::new(
            geometry.info,
            geometry.lanes,
            geometry.stripe_rows,
            geometry.group_columns,
        )?;
        session.adapter_work(definition.len() as u128 + 1)?;
        let mut r = ByteReader::new(definition);
        let present = r.varint()?;
        if present & !3 != 0 {
            return Err(Error::Unsupported("V4 numeric definition fields"));
        }
        let bank = if bank_id == 0 {
            None
        } else {
            Some(crate::seeds::bank(bank_id).ok_or(Error::Unsupported("unknown V4 model bank"))?)
        };
        // Adapt into the shared checked description, without probing for an old
        // grammar in the input. Only setup-derived geometry is inserted.
        let capacity = definition.len().checked_add(21).ok_or(Error::TooLarge)?;
        let temporary = session.v4_working(capacity as u128)?;
        let mut head = Vec::new();
        head.try_reserve_exact(capacity)
            .map_err(|_| Error::TooLarge)?;
        put_varint(&mut head, present);
        head.push(geometry.lanes);
        put_varint(&mut head, u64::from(geometry.stripe_rows));
        put_varint(&mut head, u64::from(geometry.group_columns));
        head.extend_from_slice(&definition[r.pos..]);
        let admission = session.v4_models();
        admission.check()?;
        let mut input = ByteReader::new(&head);
        let parsed =
            crate::v1::parse_head_nested(geometry.info, &mut input, &admission, bank, declared)?;
        if input.remaining() != 0 {
            return Err(Error::Corrupt("V4 numeric definition trailing bytes"));
        }
        if let Some(profile) = session.v4_profile() {
            profile.check_numeric(&parsed)?;
        }
        let output_map = crate::v1::OutMap::new(&parsed.info, &parsed.layout);
        drop(head);
        drop(temporary);
        Ok(Self {
            parsed,
            output_map,
            checks: None,
            _charge: scope.commit()?,
        })
    }

    pub fn info(&self) -> &ImageInfo {
        &self.parsed.info
    }

    pub fn checks(&self) -> super::Checks<'_> {
        self.checks
            .as_ref()
            .map_or_else(super::Checks::default, |checks| checks.get())
    }

    pub(crate) fn check_config(&self) -> Option<&alloc::sync::Arc<super::OwnedChecks>> {
        self.checks.as_ref()
    }
    #[cfg(feature = "lab")]
    pub(crate) fn inspection_maps(&self) -> Vec<(alloc::string::String, u8)> {
        self.parsed
            .layout
            .maps
            .iter()
            .map(|map| (alloc::format!("{:?}", map.mode), map.bits))
            .collect()
    }

    /// The same deterministic reconstruction bound is used by complete-input
    /// admission and by individual caller-fed jobs. No entropy is decoded here.
    pub(crate) fn execution_cost(&self, rect: crate::planes::Rect) -> Result<(u128, u128), Error> {
        let p = &self.parsed;
        let width = u128::from(rect.width());
        let rows = u128::from(rect.height());
        let row = width * p.bpp() as u128;
        if p.quick {
            let work = width * rows * (1 + p.bpp() as u128)
                + if p.quick_bits {
                    width * rows * p.bpp() as u128 * 16 + p.bpp() as u128 * 2048
                } else {
                    0
                };
            return Ok((
                crate::quick::state_bytes(&p.info, width, p.quick_bits)? + row,
                work + rows,
            ));
        }
        let physical = width.div_ceil(p.lanes as u128) * 16;
        let slots = p.layout.slot_channel.len() as u128;
        let state = crate::decoder_memory::lossless_state_bytes(
            physical,
            slots,
            if p.narrow() { 2 } else { 4 },
            rows,
            p.pal.is_some(),
            p.bpp() as u128,
            width,
        )?;
        Ok((state + row, physical * rows * (slots + 1) + rows))
    }

    /// Geometry-derived group count; no allocation depends on this count.
    pub fn group_count(&self) -> u64 {
        self.parsed.stripe_count() as u64 * self.parsed.ngroups() as u64
    }

    /// Rectangle in the numeric plane, not the final image's pixel coordinates.
    pub fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        if id >= self.group_count() {
            return Err(Error::Corrupt("V4 numeric group identity"));
        }
        let p = &self.parsed;
        let groups = p.ngroups() as u64;
        let (x, width, _) = p.group(usize::try_from(id % groups).map_err(|_| Error::TooLarge)?);
        let y = usize::try_from(
            (id / groups)
                .checked_mul(p.hs as u64)
                .ok_or(Error::TooLarge)?,
        )
        .map_err(|_| Error::TooLarge)?;
        let rows = p.hs.min(p.info.height as usize - y);
        Ok(crate::planes::Rect::new(
            x as u32,
            y as u32,
            width as u32,
            rows as u32,
        ))
    }

    /// Decode an already framed/admitted group's entropy into local strided
    /// rows. Output starts at this group's top-left, not the plane origin.
    /// The returned rectangle tells the caller where to place those rows.
    /// Other groups need no predictor rows or mutable state from this call.
    pub fn decode_group<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        entropy: &[u8],
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<crate::planes::Rect, Error> {
        let rect = self.group_rect(id)?;
        let (entropy, expected) = self
            .checks
            .as_ref()
            .map_or(Ok((entropy, &[][..])), |checks| {
                checks.split(super::CheckScope::GroupPixels, entropy)
            })?;
        self.decode_rectangle(rect, entropy, output, stride, session)?;
        self.verify_group(id, rect, output, stride, expected, session)?;
        Ok(rect)
    }

    /// Decode one coverage-derived rectangle without building positional unit
    /// arrays. Its width/origin must name one complete column group; row starts
    /// may vary, with height at most the admitted model geometry's stripe_rows.
    pub fn decode_rectangle<C: Fn() -> bool + Sync>(
        &self,
        rect: crate::planes::Rect,
        entropy: &[u8],
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        self.decode_rectangle_inner(rect, entropy, output, stride, session, false)
    }

    pub(crate) fn decode_reference<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        entropy: &[u8],
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<crate::planes::Rect, Error> {
        let rect = self.group_rect(id)?;
        let (entropy, expected) = self
            .checks
            .as_ref()
            .map_or(Ok((entropy, &[][..])), |checks| {
                checks.split(super::CheckScope::GroupPixels, entropy)
            })?;
        self.decode_rectangle_inner(rect, entropy, output, stride, session, true)?;
        self.verify_group(id, rect, output, stride, expected, session)?;
        Ok(rect)
    }

    fn verify_group<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        rect: crate::planes::Rect,
        output: &[u8],
        stride: usize,
        expected: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if let Some(checks) = &self.checks {
            let mut info = self.parsed.info.clone();
            info.width = rect.width();
            info.height = rect.height();
            checks.verify_pixels(
                super::CheckScope::GroupPixels,
                super::PixelCheckIdentity {
                    image: 0,
                    id,
                    x: rect.x(),
                    y: rect.y(),
                },
                crate::ImageRef::new(output, stride, &info),
                expected,
                session,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_rectangle_inner<C: Fn() -> bool + Sync>(
        &self,
        rect: crate::planes::Rect,
        entropy: &[u8],
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
        inter: bool,
    ) -> Result<(), Error> {
        let p = &self.parsed;
        let group = (rect.x() as usize).checked_div(p.gcols).unwrap_or(0);
        if group >= p.ngroups()
            || rect.height() == 0
            || rect.height() as usize > p.hs
            || u64::from(rect.y()) + u64::from(rect.height()) > u64::from(p.info.height)
        {
            return Err(Error::Corrupt("V4 numeric coverage rectangle"));
        }
        let (x, width, _) = p.group(group);
        if rect.x() as usize != x || rect.width() as usize != width {
            return Err(Error::Corrupt("V4 numeric coverage column group"));
        }
        let width = rect.width() as usize;
        let rows = rect.height() as usize;
        let row_bytes = width.checked_mul(p.bpp()).ok_or(Error::TooLarge)?;
        let required = (rows - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row_bytes))
            .ok_or(Error::TooLarge)?;
        if stride < row_bytes || output.len() < required {
            return Err(Error::InvalidInput("V4 numeric output extent/stride"));
        }
        let (scratch, work) = self.execution_cost(rect)?;
        let _working = session.v4_working(scratch)?;
        session.adapter_work(work - rows as u128)?;
        let mut row = Vec::new();
        row.try_reserve_exact(row_bytes)
            .map_err(|_| Error::TooLarge)?;
        row.resize(row_bytes, 0);
        if inter {
            return crate::v1::rectangle_into_reference(
                p,
                &self.output_map,
                group,
                rows,
                entropy,
                &mut row,
                output,
                stride,
                &|| session.adapter_work(1),
            );
        }
        let mut y = 0;
        crate::v1::rectangle_rows(
            p,
            &self.output_map,
            group,
            rows,
            entropy,
            &mut row,
            |row| {
                output[y * stride..y * stride + row_bytes].copy_from_slice(row);
                y += 1;
                Ok(())
            },
            &|| session.adapter_work(1),
        )?;
        Ok(())
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests;
