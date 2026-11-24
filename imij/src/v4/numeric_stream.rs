//! Bounded numeric payload writing and caller-fed tile/group decoding.
//! The outer version-4 file/chunk and final directory adapters are separate.

use alloc::sync::Arc;
#[cfg(feature = "encode")]
use alloc::vec::Vec;

#[cfg(feature = "encode")]
use super::Group;
#[cfg(feature = "encode")]
use super::TileWriter;
use super::{
    Entry, EntryKind, Groups, LocatedEntry, NumericModel, ObjectIntegrity, SetupContext,
    SpatialTile, TileEvent, TileExtent, TileLayout, TileLimits, TileOrder, TileReader,
};
use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Error, ImageInfo};

/// Spatial configuration for a numeric payload. The existing RowGroups selects
/// coding-group pixel dimensions; these dimensions group those coding groups.
#[derive(Clone, Copy, Debug)]
pub struct SpatialConfig {
    pub tile_columns: u32,
    pub tile_rows: u32,
    pub order: TileOrder,
    pub integrity: ObjectIntegrity,
    pub limits: TileLimits,
}

fn group_grid(info: &ImageInfo, columns: u32, rows: u32) -> (u32, u32) {
    (
        if columns == 0 {
            1
        } else {
            info.width.div_ceil(columns)
        },
        info.height.div_ceil(rows),
    )
}

fn located(extent: super::ObjectExtent, kind: EntryKind, id: u64) -> LocatedEntry {
    LocatedEntry {
        entry: Entry {
            kind,
            role: 0,
            id,
            record_len: extent.len,
        },
        offset: extent.offset,
        tile_id: extent.tile_id,
    }
}

/// One-pass fixed-model lossless/Quick payload encoder. No raster or per-image
/// index is retained. A supplier follows next_group(), so it can serve groups
/// directly in tile-major disk order or use explicit caller-owned reorder scratch.
///
/// Start by publishing the returned setup bytes. Publish/drain each output object
/// and spool its returned directory entry before advancing. There is no outer
/// header/chunk or final directory in this component yet.
#[cfg(feature = "encode")]
pub struct NumericGroupEncoder {
    core: crate::stream_encode::NativeGroupEncodeSession,
    info: ImageInfo,
    columns: u32,
    rows: u32,
    layout: TileLayout,
    frames: TileWriter,
    next_tile: u64,
    groups: Option<Groups>,
    next: Option<Group>,
    features: crate::Features,
    checks: Option<Arc<super::OwnedChecks>>,
    tile_pixels: Vec<u8>,
    buffer_tiles: bool,
}

#[cfg(feature = "encode")]
impl NumericGroupEncoder {
    /// The working budget reserves one framed object plus 256 KiB of setup
    /// capacity before admitting the existing numeric encoder. Source, destination
    /// and caller index/reorder scratch are separately owned. No RSS claim.
    pub fn new(
        info: ImageInfo,
        config: &crate::ImiEncoderConfig,
        spatial: SpatialConfig,
    ) -> Result<(Self, Vec<u8>), Error> {
        Self::with_placement(info, config, spatial, super::DirectoryPlacement::None)
    }

    /// Enable declared built-in checks. A tile raster is buffered only when
    /// tile checks are requested, and admitted against max_working_bytes.
    pub fn new_checked(
        info: ImageInfo,
        config: &crate::ImiEncoderConfig,
        spatial: SpatialConfig,
        checks: super::Checks<'_>,
    ) -> Result<(Self, Vec<u8>), Error> {
        Self::with_placement_checked(
            info,
            config,
            spatial,
            super::DirectoryPlacement::None,
            checks,
            true,
        )
    }

    pub(super) fn with_placement(
        info: ImageInfo,
        config: &crate::ImiEncoderConfig,
        spatial: SpatialConfig,
        placement: super::DirectoryPlacement,
    ) -> Result<(Self, Vec<u8>), Error> {
        Self::with_placement_checked(
            info,
            config,
            spatial,
            placement,
            super::Checks::default(),
            true,
        )
    }

    pub(super) fn with_placement_checked(
        info: ImageInfo,
        config: &crate::ImiEncoderConfig,
        spatial: SpatialConfig,
        placement: super::DirectoryPlacement,
        checks: super::Checks<'_>,
        buffer_tiles: bool,
    ) -> Result<(Self, Vec<u8>), Error> {
        for scope in [
            super::CheckScope::GroupPixels,
            super::CheckScope::TilePixels,
            super::CheckScope::EncodedObject,
        ] {
            if checks.list(scope).iter().any(|d| d.algorithm().is_none()) {
                return Err(Error::Unsupported("V4 encoder integrity suite"));
            }
        }
        let checks = checks.owned()?;
        let mut groups = crate::stream_encode::GroupEncodeConfig::required(config)?;
        let framed =
            usize::try_from(spatial.limits.max_object_bytes).map_err(|_| Error::TooLarge)?;
        groups.max_working_bytes = groups
            .max_working_bytes
            .checked_sub(framed)
            .and_then(|n| n.checked_sub(256 << 10))
            .ok_or(Error::TooLarge)?;
        if info.width == 0 || info.height == 0 || groups.columns == 0 || groups.rows == 0 {
            return Err(Error::InvalidInput("V4 numeric group geometry"));
        }
        let (grid_x, grid_y) = group_grid(&info, groups.columns, groups.rows);
        let layout = TileLayout::new(
            grid_x,
            grid_y,
            spatial.tile_columns,
            spatial.tile_rows,
            spatial.order,
        )?;
        if let Some(checks) = &checks {
            let first = layout.tile_at(0)?;
            let width = (u64::from(first.groups_wide) * u64::from(groups.columns))
                .min(u64::from(info.width)) as usize;
            let height = (u64::from(first.groups_high) * u64::from(groups.rows))
                .min(u64::from(info.height)) as usize;
            let tile_bytes = if buffer_tiles && checks.width(super::CheckScope::TilePixels) != 0 {
                width
                    .checked_mul(height)
                    .and_then(|n| n.checked_mul(info.bytes_per_pixel()))
                    .ok_or(Error::TooLarge)?
            } else {
                0
            };
            let reserve = tile_bytes
                .checked_add(checks.bytes().len())
                .and_then(|n| {
                    let scopes = checks.get();
                    n.checked_add(
                        super::PixelChecks::working_bytes(
                            scopes.list(super::CheckScope::GroupPixels),
                        )
                        .max(super::PixelChecks::working_bytes(
                            scopes.list(super::CheckScope::TilePixels),
                        )) + 108,
                    )
                })
                .ok_or(Error::TooLarge)?;
            groups.max_working_bytes = groups
                .max_working_bytes
                .checked_sub(reserve)
                .ok_or(Error::TooLarge)?;
        }
        let (mut core, head) =
            crate::stream_encode::NativeGroupEncodeSession::nested(info.clone(), groups)?;
        core.consume_output(0)?; // Drop the consumed V3 wrapper bytes, retain capacity.
        if head.len() > 64 << 10 {
            return Err(Error::TooLarge);
        }
        let mut body = Vec::new();
        body.try_reserve_exact(head.len().checked_add(40).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        for value in [
            u64::from(spatial.tile_columns),
            u64::from(spatial.tile_rows),
            spatial.order.wire_value(),
            spatial.integrity.wire_value(),
        ] {
            crate::put_varint(&mut body, value);
        }
        body.extend_from_slice(&head);
        if let Some(checks) = &checks {
            body.try_reserve_exact(checks.bytes().len())
                .map_err(|_| Error::TooLarge)?;
            body.extend_from_slice(checks.bytes());
        }
        let context =
            SetupContext::from_image_declaration(&info, crate::Coding::Lossless, None, &body)?;
        let mut setup = Vec::new();
        setup
            .try_reserve_exact(body.len() + 10)
            .map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut setup, body.len() as u64);
        setup.extend_from_slice(&body);
        super::append_directory_prefix(&mut setup, placement, 0, None, 1)?;
        let features = crate::v1::head_features(&info, &head);
        let frames = TileWriter::new(layout, spatial.integrity, context, spatial.limits)
            .with_checks(checks.clone(), crate::registry::UNIT_STRIPE);
        Ok((
            Self {
                core,
                info,
                columns: groups.columns,
                rows: groups.rows,
                layout,
                frames,
                next_tile: 0,
                groups: None,
                next: None,
                features,
                checks,
                tile_pixels: Vec::new(),
                buffer_tiles,
            },
            setup,
        ))
    }

    pub const fn layout(&self) -> TileLayout {
        self.layout
    }
    pub(super) const fn features(&self) -> crate::Features {
        self.features
    }
    pub(super) fn image_info(&self) -> &ImageInfo {
        &self.info
    }

    pub fn output(&self) -> &[u8] {
        self.frames.output()
    }
    pub fn consume_output(&mut self, count: usize) -> Result<(), Error> {
        self.frames.consume_output(count)
    }

    pub fn begin_tile(&mut self) -> Result<LocatedEntry, Error> {
        if self.groups.is_some() {
            return Err(Error::InvalidInput("V4 numeric tile already open"));
        }
        let tile = self.layout.tile_at(self.next_tile)?;
        if self.buffer_tiles
            && self
                .checks
                .as_ref()
                .is_some_and(|checks| checks.width(super::CheckScope::TilePixels) != 0)
        {
            let rect = self.tile_rect(tile.id)?;
            let size = (rect.width as usize)
                .checked_mul(rect.height as usize)
                .and_then(|n| n.checked_mul(self.info.bytes_per_pixel()))
                .ok_or(Error::TooLarge)?;
            self.tile_pixels.clear();
            self.tile_pixels
                .try_reserve_exact(size)
                .map_err(|_| Error::TooLarge)?;
            self.tile_pixels.resize(size, 0);
        }
        let extent = self.frames.begin(&[])?;
        let mut groups = self.layout.groups(tile.id)?;
        self.next = groups.next();
        self.groups = Some(groups);
        Ok(located(extent, EntryKind::TileBegin, tile.id))
    }

    /// Source rectangle required next. None while output waits for drainage,
    /// outside a tile, or after all groups in that tile were supplied.
    pub fn next_group(&self) -> Option<crate::stream_encode::GroupRect> {
        if !self.output().is_empty() {
            return None;
        }
        let group = self.next?;
        let x = group.x * self.columns;
        let y = group.y * self.rows;
        Some(crate::stream_encode::GroupRect {
            x,
            y,
            width: self.columns.min(self.info.width - x),
            height: self.rows.min(self.info.height - y),
        })
    }

    /// Encode exactly the requested local strided rectangle. A successful call
    /// publishes one complete V4 kind-59 object before any later input is needed.
    pub fn push_group(&mut self, pixels: &[u8], stride: usize) -> Result<LocatedEntry, Error> {
        let rect = self.next_group().ok_or(Error::InvalidInput(
            "V4 numeric input needs output drainage or a tile",
        ))?;
        let id = self.next.unwrap().id;
        let (encoded, start) = self.core.encode_rectangle(rect, pixels, stride)?;
        let mut old = ByteReader::new(&encoded[start..]);
        if old.varint()? != crate::registry::UNIT_STRIPE {
            return Err(Error::Corrupt("numeric encoder record kind"));
        }
        let length = old.var_usize()?;
        let entropy = old.bytes(length)?;
        if old.remaining() != 0 {
            return Err(Error::Corrupt("numeric encoder record extent"));
        }
        let mut identity = Vec::new();
        identity
            .try_reserve_exact(10)
            .map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut identity, id);
        let mut local = self.info.clone();
        local.width = rect.width;
        local.height = rect.height;
        let results = self.checks.as_ref().map_or(Ok(Vec::new()), |checks| {
            checks.pixels(
                super::CheckScope::GroupPixels,
                super::PixelCheckIdentity {
                    image: 0,
                    id,
                    x: rect.x,
                    y: rect.y,
                },
                crate::ImageRef::new(pixels, stride, &local),
            )
        })?;
        if !self.tile_pixels.is_empty() {
            let tile = self.tile_rect(self.layout.tile_at(self.next_tile)?.id)?;
            let bpp = self.info.bytes_per_pixel();
            let target_stride = tile.width as usize * bpp;
            for y in 0..rect.height as usize {
                let at = (rect.y as usize - tile.y as usize + y) * target_stride
                    + (rect.x as usize - tile.x as usize) * bpp;
                self.tile_pixels[at..at + rect.width as usize * bpp]
                    .copy_from_slice(&pixels[y * stride..y * stride + rect.width as usize * bpp]);
            }
        }
        let extent = self.frames.append_parts_checked(
            crate::registry::UNIT_STRIPE,
            &[&identity, entropy],
            &results,
        )?;
        self.next = self.groups.as_mut().unwrap().next();
        Ok(located(extent, EntryKind::Unit, id))
    }

    pub fn end_tile(&mut self) -> Result<(LocatedEntry, TileExtent), Error> {
        self.end_tile_inner(None)
    }

    /// Use an existing complete tile raster without making a second pixel copy.
    pub fn end_tile_with_pixels(
        &mut self,
        pixels: crate::ImageRef<'_>,
    ) -> Result<(LocatedEntry, TileExtent), Error> {
        self.end_tile_inner(Some(pixels))
    }

    fn end_tile_inner(
        &mut self,
        supplied: Option<crate::ImageRef<'_>>,
    ) -> Result<(LocatedEntry, TileExtent), Error> {
        if self.groups.is_none() || self.next.is_some() {
            return Err(Error::InvalidInput("V4 numeric tile has missing groups"));
        }
        let id = self.layout.tile_at(self.next_tile)?.id;
        let results = if let Some(checks) = &self.checks
            && checks.width(super::CheckScope::TilePixels) != 0
        {
            let rect = self.tile_rect(id)?;
            let mut info = self.info.clone();
            info.width = rect.width;
            info.height = rect.height;
            let pixels = supplied.unwrap_or_else(|| {
                crate::ImageRef::new(&self.tile_pixels, info.row_bytes(), &info)
            });
            if pixels.info != &info {
                return Err(Error::InvalidInput("V4 tile pixel interpretation"));
            }
            checks.pixels(
                super::CheckScope::TilePixels,
                super::PixelCheckIdentity {
                    image: 0,
                    id,
                    x: rect.x,
                    y: rect.y,
                },
                pixels,
            )?
        } else {
            Vec::new()
        };
        let (extent, tile) = self.frames.end_checked(&results)?;
        self.groups = None;
        self.next_tile += 1;
        self.tile_pixels.clear();
        Ok((located(extent, EntryKind::TileEnd, tile.tile_id), tile))
    }

    /// Clipped native raster covered by a spatial tile.
    pub(crate) fn tile_rect(&self, id: u64) -> Result<crate::stream_encode::GroupRect, Error> {
        let tile = self.layout.tile(id)?;
        let x = tile.group_x * self.columns;
        let y = tile.group_y * self.rows;
        Ok(crate::stream_encode::GroupRect {
            x,
            y,
            width: (u64::from(tile.groups_wide) * u64::from(self.columns))
                .min(u64::from(self.info.width - x)) as u32,
            height: (u64::from(tile.groups_high) * u64::from(self.rows))
                .min(u64::from(self.info.height - y)) as u32,
        })
    }

    /// All tiles, groups and output complete; directory/file completion is separate.
    pub fn finish(&self) -> Result<(), Error> {
        self.frames.finish()
    }
}

/// Independent reconstruction work borrowing only its encoded input. Owning the
/// immutable model handle lets the feed parser advance while workers decode.
pub struct NumericWork<'a> {
    model: Arc<NumericModel>,
    id: u64,
    entropy: &'a [u8],
    pub entry: LocatedEntry,
    /// Local object integrity only; a tile/file may still fail later validation.
    pub integrity_verified: bool,
}

impl NumericWork<'_> {
    #[cfg(feature = "lab")]
    pub(crate) fn inspection_maps(&self) -> Vec<(alloc::string::String, u8)> {
        self.model.inspection_maps()
    }
    pub(crate) fn store(
        self,
        input: &[u8],
    ) -> Result<(Arc<NumericModel>, u64, core::ops::Range<usize>), Error> {
        Ok((
            self.model,
            self.id,
            crate::owned::span(input, self.entropy)?,
        ))
    }
    pub fn rect(&self) -> Result<crate::planes::Rect, Error> {
        self.model.group_rect(self.id)
    }
    pub(crate) fn decode_reference<C: Fn() -> bool + Sync>(
        &self,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<crate::planes::Rect, Error> {
        self.model
            .decode_reference(self.id, self.entropy, output, stride, session)
    }
    pub fn decode_into<C: Fn() -> bool + Sync>(
        &self,
        output: &mut [u8],
        stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<crate::planes::Rect, Error> {
        self.model
            .decode_group(self.id, self.entropy, output, stride, session)
    }
}

/// Numeric payload delivery. Pixels remain provisional until their required
/// tile/directory/file scopes complete. This component never claims whole-file
/// validation from an accepted object or tile close.
#[non_exhaustive]
pub enum NumericEvent<'a> {
    Begin {
        tile: SpatialTile,
        entry: LocatedEntry,
    },
    Group(NumericWork<'a>),
    End {
        tile: TileExtent,
        entry: LocatedEntry,
        pixel_results: &'a [u8],
    },
    Ancillary {
        kind: u64,
        body: &'a [u8],
        entry: LocatedEntry,
    },
}

impl NumericEvent<'_> {
    pub const fn entry(&self) -> LocatedEntry {
        match self {
            Self::Begin { entry, .. } | Self::End { entry, .. } | Self::Ancillary { entry, .. } => {
                *entry
            }
            Self::Group(work) => work.entry,
        }
    }
}

/// Caller-fed numeric payload reader. Complete objects are supplied individually
/// or as a prefix of a larger available buffer; no later input or EOF is needed
/// to obtain a work item. The input owner keeps encoded bytes live during work.
pub struct NumericGroupDecoder<'s, C> {
    model: Arc<NumericModel>,
    frames: TileReader<'s, C>,
    layout: TileLayout,
    groups: Option<Groups>,
    ignorable: u64,
    failed: bool,
    integrity: ObjectIntegrity,
}

pub(crate) struct NumericState {
    model: Arc<NumericModel>,
    frames: super::tile::TileState,
    layout: TileLayout,
    #[cfg(feature = "std")]
    groups: Option<Groups>,
    #[cfg(feature = "std")]
    ignorable: u64,
    #[cfg(feature = "std")]
    failed: bool,
    integrity: ObjectIntegrity,
}
impl NumericState {
    #[cfg(feature = "std")]
    pub(crate) fn bind<C: Fn() -> bool + Sync>(
        self,
        session: &DecodeSession<C>,
    ) -> NumericGroupDecoder<'_, C> {
        NumericGroupDecoder {
            model: self.model,
            frames: self.frames.bind(session),
            layout: self.layout,
            groups: self.groups,
            ignorable: self.ignorable,
            failed: self.failed,
            integrity: self.integrity,
        }
    }
    pub(crate) fn at_tile<'s, C: Fn() -> bool + Sync>(
        &self,
        session: &'s DecodeSession<C>,
        id: u64,
        offset: u64,
        ignorable: u64,
    ) -> Result<NumericGroupDecoder<'s, C>, Error> {
        Ok(NumericGroupDecoder {
            model: Arc::clone(&self.model),
            frames: self
                .frames
                .at_tile(session, self.layout.tile_ordinal(id)?, offset)?,
            layout: self.layout,
            groups: None,
            ignorable,
            failed: false,
            integrity: self.integrity,
        })
    }
    pub(crate) fn check_config(&self) -> Option<&Arc<super::OwnedChecks>> {
        self.model.check_config()
    }
    pub(crate) fn integrity(&self) -> ObjectIntegrity {
        self.integrity
    }
    pub(crate) fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        self.model.group_rect(id)
    }
    pub(crate) fn layout(&self) -> TileLayout {
        self.layout
    }
}

impl<'s, C: Fn() -> bool + Sync> NumericGroupDecoder<'s, C> {
    pub(crate) fn into_state(self) -> NumericState {
        NumericState {
            model: self.model,
            frames: self.frames.into_state(),
            layout: self.layout,
            #[cfg(feature = "std")]
            groups: self.groups,
            #[cfg(feature = "std")]
            ignorable: self.ignorable,
            #[cfg(feature = "std")]
            failed: self.failed,
            integrity: self.integrity,
        }
    }

    /// Parse the immutable setup and common directory-placement prefix. Returned
    /// length is the first object's offset within DATA. The enclosing file or
    /// sparse driver owns directory reconciliation and finalization.
    pub fn new(
        info: ImageInfo,
        declared: crate::Features,
        input: &[u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<(Self, usize), Error> {
        Self::with_directory(info, declared, input, limits, session)
            .map(|(decoder, used, _)| (decoder, used))
    }

    pub(crate) fn with_directory<'a>(
        info: ImageInfo,
        declared: crate::Features,
        input: &'a [u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<(Self, usize, super::DirectoryPrefix<'a>), Error> {
        let mut prefix = ByteReader::new(input);
        crate::trace::trace_enter_addr("payload", "V4:2.1", "numeric setup", prefix.mark());
        let length = crate::trace::tf!("setup bytes", prefix, prefix.var_usize()?);
        let total = prefix.pos.checked_add(length).ok_or(Error::TooLarge)?;
        session.v4_setup(total)?;
        let body = prefix.bytes(length)?;
        let decoder = Self::from_setup(info, declared, body, limits, session)?;
        crate::trace::exit_addr(prefix.mark());
        let placement = super::DirectoryPrefix::read(&input[total..], session)?;
        let consumed = total
            .checked_add(placement.encoded_len)
            .ok_or(Error::TooLarge)?;
        Ok((decoder, consumed, placement))
    }

    pub(super) fn from_setup(
        info: ImageInfo,
        declared: crate::Features,
        body: &[u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<Self, Error> {
        let mut setup = ByteReader::new(body);
        let tile_columns = crate::trace::tf!("tile_columns", setup, setup.var_u32()?);
        let tile_rows = crate::trace::tf!("tile_rows", setup, setup.var_u32()?);
        let order = TileOrder::try_from(crate::trace::tf!("tile order", setup, setup.varint()?))?;
        let integrity = ObjectIntegrity::try_from(crate::trace::tf!(
            "object integrity",
            setup,
            setup.varint()?
        ))?;
        let context =
            SetupContext::from_image_declaration(&info, crate::Coding::Lossless, None, body)?;
        let model = Arc::new(NumericModel::read_root(
            info,
            &body[setup.pos..],
            declared,
            session,
        )?);
        let geometry = model.geometry()?;
        let (grid_x, grid_y) =
            group_grid(&geometry.info, geometry.group_columns, geometry.stripe_rows);
        let layout = TileLayout::new(grid_x, grid_y, tile_columns, tile_rows, order)?;
        let check_config = model.check_config().cloned();
        Ok(Self {
            model,
            frames: TileReader::new(layout, integrity, context, limits, session)
                .with_checks(check_config),
            layout,
            groups: None,
            ignorable: 0,
            failed: false,
            integrity,
        })
    }

    pub fn info(&self) -> &ImageInfo {
        self.model.info()
    }
    pub(crate) fn shared_model(&self) -> Arc<NumericModel> {
        self.model.clone()
    }

    pub(crate) fn check_config(&self) -> Option<&Arc<super::OwnedChecks>> {
        self.model.check_config()
    }

    pub(crate) fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        self.model.group_rect(id)
    }

    pub const fn layout(&self) -> TileLayout {
        self.layout
    }
    pub const fn integrity(&self) -> ObjectIntegrity {
        self.integrity
    }

    /// Start with an independently fetched, directory-admitted complete tile.
    /// The End event validates that tile only; finish_structure remains the
    /// complete-image traversal gate. `first_ignorable` is the global ancillary
    /// ordinal at that position, obtained from the admitted directory prefix.
    pub fn at_tile(
        mut self,
        id: u64,
        object_offset: u64,
        first_ignorable: u64,
    ) -> Result<Self, Error> {
        self.frames = self
            .frames
            .at_tile(self.layout.tile_ordinal(id)?, object_offset)?;
        self.ignorable = first_ignorable;
        Ok(self)
    }

    /// Read exactly one available complete object and produce a group task
    /// independently of future groups. A semantic failure poisons this stream.
    pub fn read<'a>(&mut self, input: &'a [u8]) -> Result<NumericEvent<'a>, Error> {
        if self.failed {
            return Err(Error::Corrupt("failed V4 numeric stream"));
        }
        let result = self.read_inner(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_inner<'a>(&mut self, input: &'a [u8]) -> Result<NumericEvent<'a>, Error> {
        match self.frames.read(input)? {
            TileEvent::Begin { extent, exact_plan } => {
                if !exact_plan.is_empty() {
                    return Err(Error::Corrupt("exact plan on numeric tile"));
                }
                let id = extent.tile_id.unwrap();
                self.groups = Some(self.layout.groups(id)?);
                if crate::trace::on() {
                    let tile = self.layout.tile(id)?;
                    let columns = u64::from(self.layout.group_grid().0);
                    let first = self
                        .model
                        .group_rect(u64::from(tile.group_y) * columns + u64::from(tile.group_x))?;
                    let last = self.model.group_rect(
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
                Ok(NumericEvent::Begin {
                    tile: self.layout.tile(id)?,
                    entry: located(extent, EntryKind::TileBegin, id),
                })
            }
            TileEvent::Object { extent, object } => {
                if object.kind & 1 == 0 {
                    let entry = located(extent, EntryKind::Ignorable, self.ignorable);
                    self.ignorable = self.ignorable.checked_add(1).ok_or(Error::TooLarge)?;
                    return Ok(NumericEvent::Ancillary {
                        kind: object.kind,
                        body: object.body,
                        entry,
                    });
                }
                if object.kind != crate::registry::UNIT_STRIPE {
                    return Err(Error::Unsupported("non-numeric object in numeric payload"));
                }
                let mut body = ByteReader::new(object.body);
                crate::trace::trace_enter_addr("payload", "V4:2.1", "numeric group", body.mark());
                let id = crate::trace::tf!("unit ID", body, body.varint()?);
                if self.groups.as_mut().and_then(Iterator::next).map(|g| g.id) != Some(id) {
                    return Err(Error::Corrupt("V4 numeric unit schedule/identity"));
                }
                if crate::trace::on() {
                    let rect = self.model.group_rect(id)?;
                    crate::trace::region(
                        u64::from(rect.x()),
                        u64::from(rect.y()),
                        u64::from(rect.width()),
                        u64::from(rect.height()),
                    );
                }
                crate::trace::exit_addr(
                    crate::trace::Mark::Addr(object.body.as_ptr() as usize).plus(object.body.len()),
                );
                Ok(NumericEvent::Group(NumericWork {
                    model: Arc::clone(&self.model),
                    id,
                    entropy: &object.body[body.pos..],
                    entry: located(extent, EntryKind::Unit, id),
                    integrity_verified: object.integrity_verified,
                }))
            }
            TileEvent::End {
                extent,
                tile,
                pixel_results,
            } => {
                if self.groups.as_mut().and_then(Iterator::next).is_some() {
                    return Err(Error::Corrupt("V4 numeric tile missing groups"));
                }
                self.groups = None;
                Ok(NumericEvent::End {
                    tile,
                    pixel_results,
                    entry: located(extent, EntryKind::TileEnd, tile.tile_id),
                })
            }
        }
    }

    /// Full numeric tile schedule checked. Caller must separately reconcile the
    /// observed entries and complete the enclosing directory/file validation.
    pub fn finish_structure(&self) -> Result<(), Error> {
        if self.failed || self.groups.is_some() {
            return Err(Error::Corrupt("incomplete V4 numeric stream"));
        }
        self.frames.finish()
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests;
