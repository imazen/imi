//! Contiguous spatial-tile framing, independent of pixel/entropy semantics.

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::object::{append_object_parts, object_len};
use super::{Object, ObjectIntegrity, ObjectReader, SetupContext, TileLayout};
use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Error, Hasher, put_varint};

use crate::registry::MODEL_DEFINITION;
pub(super) use crate::registry::{TILE_BEGIN, TILE_END};

/// Bounds on encoded working objects and complete spatial-tile extents.
#[derive(Clone, Copy, Debug)]
pub struct TileLimits {
    /// Maximum complete framed object, including its local digest.
    pub max_object_bytes: u64,
    /// Maximum bytes from tile begin through tile end, including all digests.
    pub max_tile_bytes: u64,
}

/// One physical object's extent, relative to the first framed V4 object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectExtent {
    /// Record kind.
    pub kind: u64,
    /// First byte of the encoded record.
    pub offset: u64,
    /// Complete framed length.
    pub len: u64,
    /// Containing tile, or None for a definition outside a tile.
    pub tile_id: Option<u64>,
}

/// A complete tile range; its groups remain independently schedulable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileExtent {
    /// Geometry-derived tile identity.
    pub tile_id: u64,
    /// First byte of TILE_BEGIN.
    pub offset: u64,
    /// Bytes through the end of TILE_END.
    pub len: u64,
    /// Framed children, excluding begin and end markers.
    pub child_count: u64,
}

struct OpenTile {
    id: u64,
    start: u64,
    children: u64,
    hash: Option<Hasher>,
}

impl OpenTile {
    fn new(id: u64, start: u64, mode: ObjectIntegrity, context: SetupContext) -> Self {
        Self {
            id,
            start,
            children: 0,
            hash: (mode == ObjectIntegrity::ObjectsAndTiles).then(|| context.tile_hash()),
        }
    }
}

/// Forward-only tile framer with explicit output drainage.
///
/// Retains at most one encoded object, never a tile or image-wide index. The
/// caller spools returned extents into its index storage and must consume output
/// before submitting another object. This layer validates tile framing/order;
/// the codec supplies checked model/unit bodies and the optional exact plan.
pub struct TileWriter {
    layout: TileLayout,
    mode: ObjectIntegrity,
    context: SetupContext,
    limits: TileLimits,
    next_tile: u64,
    offset: u64,
    open: Option<OpenTile>,
    pending: Vec<u8>,
    drained: usize,
    checks: Option<Arc<super::OwnedChecks>>,
    pixel_group_kind: u64,
}

impl TileWriter {
    /// Start at the first object after the setup/directory reservation.
    pub fn new(
        layout: TileLayout,
        mode: ObjectIntegrity,
        context: SetupContext,
        limits: TileLimits,
    ) -> Self {
        Self {
            layout,
            mode,
            context,
            limits,
            next_tile: 0,
            offset: 0,
            open: None,
            pending: Vec::new(),
            drained: 0,
            checks: None,
            pixel_group_kind: 0,
        }
    }

    #[cfg(feature = "encode")]
    pub(crate) fn with_checks(
        mut self,
        checks: Option<Arc<super::OwnedChecks>>,
        pixel_group_kind: u64,
    ) -> Self {
        self.checks = checks;
        self.pixel_group_kind = pixel_group_kind;
        self
    }

    /// Bytes ready to publish now, without waiting for tile close or an index.
    pub fn output(&self) -> &[u8] {
        &self.pending[self.drained..]
    }

    /// Acknowledge bytes accepted by the destination, including partial writes.
    pub fn consume_output(&mut self, bytes: usize) -> Result<(), Error> {
        if bytes > self.output().len() {
            return Err(Error::InvalidInput(
                "V4 output consumption exceeds pending bytes",
            ));
        }
        self.drained += bytes;
        if self.drained == self.pending.len() {
            self.pending.clear();
            self.drained = 0;
        }
        Ok(())
    }

    fn ready(&self) -> Result<(), Error> {
        if !self.pending.is_empty() {
            return Err(Error::InvalidInput(
                "drain V4 output before submitting another object",
            ));
        }
        Ok(())
    }

    fn frame(&mut self, kind: u64, parts: &[&[u8]]) -> Result<ObjectExtent, Error> {
        self.frame_checked(kind, parts, &[])
    }

    fn frame_checked(
        &mut self,
        kind: u64,
        parts: &[&[u8]],
        pixel_results: &[u8],
    ) -> Result<ObjectExtent, Error> {
        self.ready()?;
        let mut extended = Vec::new();
        let encoded_results;
        let parts = if let Some(checks) = &self.checks {
            let pixel_scope = if kind == TILE_END {
                Some(super::CheckScope::TilePixels)
            } else if kind == self.pixel_group_kind {
                Some(super::CheckScope::GroupPixels)
            } else {
                None
            };
            if pixel_results.len() != pixel_scope.map_or(0, |scope| checks.width(scope)) {
                return Err(Error::InvalidInput("V4 pixel result width"));
            }
            let size = parts.iter().try_fold(pixel_results.len(), |sum, part| {
                sum.checked_add(part.len()).ok_or(Error::TooLarge)
            })?;
            let mut prefix = Vec::new();
            prefix.try_reserve_exact(20).map_err(|_| Error::TooLarge)?;
            put_varint(&mut prefix, kind);
            put_varint(
                &mut prefix,
                size.checked_add(checks.width(super::CheckScope::EncodedObject))
                    .and_then(|n| n.checked_add(self.mode.trailer_len()))
                    .ok_or(Error::TooLarge)? as u64,
            );
            encoded_results = checks.encoded_results(
                &self.context,
                [&prefix[..]]
                    .into_iter()
                    .chain(parts.iter().copied())
                    .chain(core::iter::once(pixel_results)),
            )?;
            extended
                .try_reserve_exact(parts.len() + 2)
                .map_err(|_| Error::TooLarge)?;
            extended.extend_from_slice(parts);
            extended.push(pixel_results);
            extended.push(&encoded_results);
            &extended[..]
        } else {
            if !pixel_results.is_empty() {
                return Err(Error::InvalidInput("undeclared V4 pixel results"));
            }
            parts
        };
        let size = parts.iter().try_fold(0usize, |sum, part| {
            sum.checked_add(part.len()).ok_or(Error::TooLarge)
        })?;
        let len = object_len(kind, size, self.mode)? as u64;
        if kind == TILE_BEGIN && len > self.limits.max_tile_bytes {
            return Err(Error::TooLarge);
        }
        let end = self.offset.checked_add(len).ok_or(Error::TooLarge)?;
        if let Some(tile) = &self.open
            && end - tile.start > self.limits.max_tile_bytes
        {
            return Err(Error::TooLarge);
        }
        append_object_parts(
            &mut self.pending,
            kind,
            parts,
            self.mode,
            self.context,
            self.limits.max_object_bytes,
        )?;
        let extent = ObjectExtent {
            kind,
            offset: self.offset,
            len,
            tile_id: self.open.as_ref().map(|t| t.id),
        };
        self.offset = end;
        Ok(extent)
    }

    /// Publish the next tile's opening marker. `exact_plan` is the already
    /// checked optional exact-plane suffix following its tile ID.
    pub fn begin(&mut self, exact_plan: &[u8]) -> Result<ObjectExtent, Error> {
        self.ready()?;
        if self.open.is_some() {
            return Err(Error::InvalidInput("V4 tile already open"));
        }
        let tile = self.layout.tile_at(self.next_tile)?;
        let mut id = Vec::new();
        id.try_reserve(10).map_err(|_| Error::TooLarge)?;
        put_varint(&mut id, tile.id);
        let body_len = id
            .len()
            .checked_add(exact_plan.len())
            .ok_or(Error::TooLarge)?;
        if object_len(TILE_BEGIN, body_len, self.mode)? as u64 > self.limits.max_tile_bytes {
            return Err(Error::TooLarge);
        }
        let mut extent = self.frame(TILE_BEGIN, &[&id, exact_plan])?;
        let mut open = OpenTile::new(tile.id, extent.offset, self.mode, self.context);
        if let Some(hash) = &mut open.hash {
            hash.update(&self.pending);
        }
        extent.tile_id = Some(tile.id);
        self.open = Some(open);
        Ok(extent)
    }

    /// Publish one model/unit/ancillary object. Tile markers must use begin/end.
    /// Definitions (kind 63) and ignorable even kinds may occur outside tiles.
    pub fn append(&mut self, kind: u64, body: &[u8]) -> Result<ObjectExtent, Error> {
        self.append_parts(kind, &[body])
    }

    pub(super) fn append_parts(
        &mut self,
        kind: u64,
        parts: &[&[u8]],
    ) -> Result<ObjectExtent, Error> {
        self.append_parts_checked(kind, parts, &[])
    }

    pub(super) fn append_parts_checked(
        &mut self,
        kind: u64,
        parts: &[&[u8]],
        pixel_results: &[u8],
    ) -> Result<ObjectExtent, Error> {
        self.ready()?;
        if kind == TILE_BEGIN
            || kind == TILE_END
            || (self.open.is_none() && kind != MODEL_DEFINITION && kind & 1 != 0)
        {
            return Err(Error::InvalidInput("V4 object outside its tile"));
        }
        let children = self.open.as_ref().map(|t| t.children.checked_add(1));
        if matches!(children, Some(None)) {
            return Err(Error::TooLarge);
        }
        let extent = self.frame_checked(kind, parts, pixel_results)?;
        if let Some(tile) = &mut self.open {
            tile.children = children.flatten().unwrap();
            if let Some(hash) = &mut tile.hash {
                hash.update(&self.pending);
            }
        }
        Ok(extent)
    }

    /// Close the tile with its observed length/count and optional aggregate.
    /// Returned extent covers begin, children and end as one range.
    pub fn end(&mut self) -> Result<(ObjectExtent, TileExtent), Error> {
        self.end_checked(&[])
    }

    pub(crate) fn end_checked(
        &mut self,
        pixel_results: &[u8],
    ) -> Result<(ObjectExtent, TileExtent), Error> {
        self.ready()?;
        let tile = self
            .open
            .as_ref()
            .ok_or(Error::InvalidInput("no open V4 tile"))?;
        let mut body = Vec::new();
        body.try_reserve(46).map_err(|_| Error::TooLarge)?;
        put_varint(&mut body, tile.id);
        put_varint(&mut body, self.offset - tile.start);
        put_varint(&mut body, tile.children);
        if let Some(hash) = &tile.hash {
            let mut hash = hash.clone();
            hash.update(&body);
            body.extend_from_slice(hash.finish().as_bytes());
        }
        let extent = self.frame_checked(TILE_END, &[&body], pixel_results)?;
        let tile = self.open.take().unwrap();
        self.next_tile += 1;
        Ok((
            extent,
            TileExtent {
                tile_id: tile.id,
                offset: tile.start,
                len: self.offset - tile.start,
                child_count: tile.children,
            },
        ))
    }

    /// Confirm all declared tiles closed and all pending bytes were consumed.
    /// Final directory reconciliation is a separate structural operation.
    pub fn finish(&self) -> Result<(), Error> {
        self.ready()?;
        if self.open.is_some() || self.next_tile != self.layout.tile_count() {
            return Err(Error::InvalidInput("incomplete V4 tile stream"));
        }
        Ok(())
    }
}

/// An observed tile event, without any implied entropy/pixel validation.
#[derive(Clone, Copy, Debug)]
pub enum TileEvent<'a> {
    /// Opening marker and its unparsed exact-plane suffix.
    Begin {
        extent: ObjectExtent,
        exact_plan: &'a [u8],
    },
    /// An independently framed child, or definition outside a tile.
    Object {
        extent: ObjectExtent,
        object: Object<'a>,
    },
    /// Closing marker. Local and declared aggregate integrity have passed.
    End {
        extent: ObjectExtent,
        tile: TileExtent,
        pixel_results: &'a [u8],
    },
}

/// Sequential/range tile checker. It does not retain previous object bodies.
///
/// A failure poisons the stream. A successful end proves framing and declared
/// integrity for the supplied tile bytes, not semantic unit/model validity.
pub struct TileReader<'s, C> {
    objects: ObjectReader<'s, C>,
    session: &'s DecodeSession<C>,
    layout: TileLayout,
    mode: ObjectIntegrity,
    context: SetupContext,
    limits: TileLimits,
    next_tile: u64,
    offset: u64,
    open: Option<OpenTile>,
    failed: bool,
    input_admitted: bool,
}

/// Parser state contains no borrowed request. Adapters reattach the same
/// retained-budget account while polling input or reading another range.
pub(crate) struct TileState {
    layout: TileLayout,
    mode: ObjectIntegrity,
    context: SetupContext,
    limits: TileLimits,
    checks: Option<Arc<super::OwnedChecks>>,
    #[cfg(feature = "std")]
    next_tile: u64,
    #[cfg(feature = "std")]
    offset: u64,
    #[cfg(feature = "std")]
    open: Option<OpenTile>,
    #[cfg(feature = "std")]
    failed: bool,
    #[cfg(feature = "std")]
    input_admitted: bool,
}
impl TileState {
    #[cfg(feature = "std")]
    pub(crate) fn bind<C: Fn() -> bool + Sync>(
        self,
        session: &DecodeSession<C>,
    ) -> TileReader<'_, C> {
        TileReader {
            objects: ObjectReader::new(
                self.mode,
                self.context,
                session,
                self.limits.max_object_bytes,
            )
            .with_checks(self.checks),
            session,
            layout: self.layout,
            mode: self.mode,
            context: self.context,
            limits: self.limits,
            next_tile: self.next_tile,
            offset: self.offset,
            open: self.open,
            failed: self.failed,
            input_admitted: self.input_admitted,
        }
    }
    pub(crate) fn at_tile<'s, C: Fn() -> bool + Sync>(
        &self,
        session: &'s DecodeSession<C>,
        ordinal: u64,
        offset: u64,
    ) -> Result<TileReader<'s, C>, Error> {
        let mut reader =
            TileReader::new(self.layout, self.mode, self.context, self.limits, session)
                .with_checks(self.checks.clone())
                .at_tile(ordinal, offset)?;
        reader.input_admitted = true;
        Ok(reader)
    }
}

impl<'s, C: Fn() -> bool + Sync> TileReader<'s, C> {
    pub(crate) fn with_checks(mut self, checks: Option<Arc<super::OwnedChecks>>) -> Self {
        self.objects = self.objects.with_checks(checks);
        self
    }

    pub(crate) fn into_state(self) -> TileState {
        TileState {
            layout: self.layout,
            mode: self.mode,
            context: self.context,
            limits: self.limits,
            checks: self.objects.check_config().cloned(),
            #[cfg(feature = "std")]
            next_tile: self.next_tile,
            #[cfg(feature = "std")]
            offset: self.offset,
            #[cfg(feature = "std")]
            open: self.open,
            #[cfg(feature = "std")]
            failed: self.failed,
            #[cfg(feature = "std")]
            input_admitted: self.input_admitted,
        }
    }

    /// Read the full tile stream, beginning at logical object offset zero.
    pub fn new(
        layout: TileLayout,
        mode: ObjectIntegrity,
        context: SetupContext,
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Self {
        Self {
            objects: ObjectReader::new(mode, context, session, limits.max_object_bytes),
            session,
            layout,
            mode,
            context,
            limits,
            next_tile: 0,
            offset: 0,
            open: None,
            failed: false,
            input_admitted: false,
        }
    }

    pub(crate) fn admitted(mut self) -> Self {
        self.input_admitted = true;
        self
    }

    /// Start a sparse tile-range check at a directory-admitted physical tile
    /// ordinal/offset. This does not validate the directory or earlier bytes.
    pub fn at_tile(mut self, ordinal: u64, object_offset: u64) -> Result<Self, Error> {
        if self.failed || self.open.is_some() || self.offset != 0 || self.next_tile != 0 {
            return Err(Error::InvalidInput(
                "V4 range start requires a fresh tile reader",
            ));
        }
        self.layout.tile_at(ordinal)?;
        self.next_tile = ordinal;
        self.offset = object_offset;
        Ok(self)
    }

    /// Consume one complete object. Later bytes may be absent. On error, this
    /// stream cannot be resumed; temporary input exhaustion belongs to the feed
    /// adapter and must not call this complete-object operation.
    pub fn read<'a>(&mut self, input: &'a [u8]) -> Result<TileEvent<'a>, Error> {
        if self.failed {
            return Err(Error::Corrupt("failed V4 tile stream"));
        }
        let result = self.read_inner(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_inner<'a>(&mut self, input: &'a [u8]) -> Result<TileEvent<'a>, Error> {
        // Deny an oversized declared tile extent before hashing its body.
        if let Some(tile) = &self.open {
            let mut prefix = ByteReader::new(input);
            prefix.varint()?;
            let body_len = prefix.varint()?;
            let len = body_len
                .checked_add(prefix.pos as u64)
                .ok_or(Error::TooLarge)?;
            let end = self.offset.checked_add(len).ok_or(Error::TooLarge)?;
            if end - tile.start > self.limits.max_tile_bytes {
                return Err(Error::TooLarge);
            }
        }
        let object = if self.input_admitted {
            self.objects.read_admitted(input)?
        } else {
            self.objects.read(input)?
        };
        let end = self
            .offset
            .checked_add(object.encoded_len as u64)
            .ok_or(Error::TooLarge)?;
        let mut extent = ObjectExtent {
            kind: object.kind,
            offset: self.offset,
            len: object.encoded_len as u64,
            tile_id: self.open.as_ref().map(|t| t.id),
        };
        if let Some(tile) = &self.open
            && end - tile.start > self.limits.max_tile_bytes
        {
            return Err(Error::TooLarge);
        }
        let event = match object.kind {
            TILE_BEGIN => {
                if self.open.is_some() {
                    return Err(Error::Corrupt("nested V4 tile"));
                }
                if object.encoded_len as u64 > self.limits.max_tile_bytes {
                    return Err(Error::TooLarge);
                }
                let mut r = ByteReader::new(object.body);
                let id = crate::trace::tf!("tile ID", r, r.varint()?);
                if id != self.layout.tile_at(self.next_tile)?.id {
                    return Err(Error::Corrupt("V4 tile physical order"));
                }
                extent.tile_id = Some(id);
                self.open = Some(OpenTile::new(id, self.offset, self.mode, self.context));
                crate::trace::trace_enter_addr(
                    "tile",
                    "V4:5.1",
                    "spatial tile",
                    crate::trace::Mark::Addr(input.as_ptr() as usize),
                );
                crate::trace::tnote!("tile ID", alloc::format!("{id}"));
                self.hash_bytes(&input[..object.encoded_len])?;
                TileEvent::Begin {
                    extent,
                    exact_plan: &object.body[r.pos..],
                }
            }
            TILE_END => {
                let tile = self
                    .open
                    .as_ref()
                    .ok_or(Error::Corrupt("V4 tile end without begin"))?;
                let (body, pixel_results) = self
                    .objects
                    .check_config()
                    .map_or(Ok((object.body, &[][..])), |checks| {
                        checks.split(super::CheckScope::TilePixels, object.body)
                    })?;
                let mut r = ByteReader::new(body);
                let id = crate::trace::tf!("tile ID", r, r.varint()?);
                let bytes = crate::trace::tf!("bytes before tile end", r, r.varint()?);
                let count = crate::trace::tf!("child objects", r, r.varint()?);
                if id != tile.id || bytes != self.offset - tile.start || count != tile.children {
                    return Err(Error::Corrupt("V4 tile close identity/extent/count"));
                }
                if let Some(hash) = &tile.hash {
                    self.session.adapter_work(r.pos as u128)?;
                    let mut hash = hash.clone();
                    hash.update(&body[..r.pos]);
                    if r.bytes(16)? != hash.finish().as_bytes() {
                        return Err(Error::Corrupt("V4 tile checksum mismatch"));
                    }
                }
                if r.pos != body.len() {
                    return Err(Error::Corrupt("V4 tile end trailing bytes"));
                }
                let tile = self.open.take().unwrap();
                self.next_tile += 1;
                crate::trace::exit_addr(
                    crate::trace::Mark::Addr(input.as_ptr() as usize).plus(object.encoded_len),
                );
                TileEvent::End {
                    extent,
                    pixel_results,
                    tile: TileExtent {
                        tile_id: tile.id,
                        offset: tile.start,
                        len: end - tile.start,
                        child_count: tile.children,
                    },
                }
            }
            _ => {
                if let Some(tile) = &mut self.open {
                    tile.children = tile.children.checked_add(1).ok_or(Error::TooLarge)?;
                    self.hash_bytes(&input[..object.encoded_len])?;
                } else if object.kind != MODEL_DEFINITION && object.kind & 1 != 0 {
                    return Err(Error::Corrupt("V4 object outside its tile"));
                }
                TileEvent::Object { extent, object }
            }
        };
        self.offset = end;
        Ok(event)
    }

    fn hash_bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if let Some(hash) = self.open.as_mut().and_then(|t| t.hash.as_mut()) {
            for chunk in bytes.chunks(64 * 1024) {
                self.session.adapter_work(chunk.len() as u128)?;
                hash.update(chunk);
            }
        }
        Ok(())
    }

    /// Check full traversal completion. Sparse tile readers use their checked
    /// End event/extent and need not claim completion for the entire file.
    pub fn finish(&self) -> Result<(), Error> {
        if self.failed || self.open.is_some() || self.next_tile != self.layout.tile_count() {
            return Err(Error::Corrupt("incomplete V4 tile stream"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use crate::v4::{TileOrder, append_object};
    use alloc::vec;

    fn layout() -> TileLayout {
        TileLayout::new(4, 4, 2, 2, TileOrder::Morton).unwrap()
    }

    fn limits() -> TileLimits {
        TileLimits {
            max_object_bytes: 4096,
            max_tile_bytes: 32768,
        }
    }

    fn context() -> SetupContext {
        SetupContext::from_declaration(b"image setup")
    }

    fn drain(w: &mut TileWriter, output: &mut Vec<u8>) {
        output.extend_from_slice(w.output());
        w.consume_output(w.output().len()).unwrap();
    }

    fn fixture(mode: ObjectIntegrity) -> (Vec<u8>, Vec<TileExtent>) {
        let mut w = TileWriter::new(layout(), mode, context(), limits());
        let mut bytes = Vec::new();
        let mut tiles = Vec::new();
        w.append(63, b"root model").unwrap();
        drain(&mut w, &mut bytes);
        for ordinal in 0..layout().tile_count() {
            let tile = layout().tile_at(ordinal).unwrap();
            w.begin(&[]).unwrap();
            drain(&mut w, &mut bytes);
            for group in layout().groups(tile.id).unwrap() {
                // Opaque unit bodies for this framing-layer witness, not pixels.
                w.append(53, &group.id.to_le_bytes()).unwrap();
                drain(&mut w, &mut bytes);
            }
            tiles.push(w.end().unwrap().1);
            drain(&mut w, &mut bytes);
        }
        w.finish().unwrap();
        (bytes, tiles)
    }

    #[test]
    fn tile_extent_is_one_range_with_independent_group_records() {
        for mode in [
            ObjectIntegrity::None,
            ObjectIntegrity::Objects,
            ObjectIntegrity::ObjectsAndTiles,
        ] {
            let (bytes, tiles) = fixture(mode);
            let session = DecodeSession::new(DecodePolicy::default());
            let mut reader = TileReader::new(layout(), mode, context(), limits(), &session);
            let mut offset = 0;
            let mut ids = Vec::new();
            let mut ends = Vec::new();
            while offset < bytes.len() {
                let event = reader.read(&bytes[offset..]).unwrap();
                let extent = match event {
                    TileEvent::Object { extent, object } => {
                        if object.kind == 53 {
                            ids.push(u64::from_le_bytes(object.body.try_into().unwrap()));
                        }
                        extent
                    }
                    TileEvent::Begin { extent, exact_plan } => {
                        assert!(exact_plan.is_empty());
                        extent
                    }
                    TileEvent::End { extent, tile, .. } => {
                        ends.push(tile);
                        extent
                    }
                };
                assert_eq!(extent.offset, offset as u64);
                offset += extent.len as usize;
            }
            reader.finish().unwrap();
            assert_eq!(ids, [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15]);
            assert_eq!(ends, tiles);
            for (ordinal, tile) in tiles.iter().enumerate() {
                // Simulates exactly one admitted range fetch, no preceding bytes.
                let range = &bytes[tile.offset as usize..(tile.offset + tile.len) as usize];
                let session = DecodeSession::new(DecodePolicy::default());
                let mut reader = TileReader::new(layout(), mode, context(), limits(), &session)
                    .at_tile(ordinal as u64, tile.offset)
                    .unwrap();
                let mut cursor = 0;
                let mut count = 0;
                while cursor < range.len() {
                    let event = reader.read(&range[cursor..]).unwrap();
                    cursor += match event {
                        TileEvent::Object { extent, .. } => {
                            count += 1;
                            extent.len as usize
                        }
                        TileEvent::Begin { extent, .. } => extent.len as usize,
                        TileEvent::End {
                            extent,
                            tile: observed,
                            ..
                        } => {
                            assert_eq!(&observed, tile);
                            extent.len as usize
                        }
                    };
                }
                assert_eq!(count, 4);
            }
        }
    }

    #[test]
    fn tile_digest_matches_independent_oracle_over_exact_wire_bytes() {
        let (bytes, tiles) = fixture(ObjectIntegrity::ObjectsAndTiles);
        let tile = tiles[0];
        let session = DecodeSession::new(DecodePolicy::default());
        let reader = ObjectReader::new(ObjectIntegrity::ObjectsAndTiles, context(), &session, 4096);
        let mut end_offset = tile.offset as usize;
        let end = loop {
            let object = reader.read(&bytes[end_offset..]).unwrap();
            if object.kind == TILE_END {
                break object;
            }
            end_offset += object.encoded_len;
        };
        let fields = &end.body[..end.body.len() - 16];
        let mut oracle = b"IMI/v4/tile\0".to_vec();
        oracle.extend_from_slice(context().as_bytes());
        oracle.extend_from_slice(&bytes[tile.offset as usize..end_offset]);
        oracle.extend_from_slice(fields);
        assert_eq!(
            &end.body[fields.len()..],
            xxhash_rust::xxh3::xxh3_128(&oracle).to_le_bytes()
        );
    }

    #[test]
    fn output_can_be_partially_drained_before_next_group_or_tile_close() {
        let mode = ObjectIntegrity::ObjectsAndTiles;
        let mut w = TileWriter::new(layout(), mode, context(), limits());
        let session = DecodeSession::new(DecodePolicy::default());
        let mut reader = TileReader::new(layout(), mode, context(), limits(), &session);
        w.begin(&[]).unwrap();
        assert!(matches!(
            reader.read(w.output()).unwrap(),
            TileEvent::Begin { .. }
        ));
        let n = w.output().len();
        assert!(w.consume_output(n + 1).is_err());
        w.consume_output(1).unwrap();
        assert!(w.append(53, b"first group").is_err());
        w.consume_output(n - 1).unwrap();
        w.append(53, b"first group").unwrap();
        assert!(matches!(
            reader.read(w.output()).unwrap(),
            TileEvent::Object {
                object: Object {
                    body: b"first group",
                    ..
                },
                ..
            }
        ));
        // No second group, tile end, index or EOF has been supplied.
        assert!(reader.finish().is_err());
        assert!(w.finish().is_err());
    }

    #[test]
    fn close_binds_counts_lengths_order_and_the_whole_tile() {
        let mode = ObjectIntegrity::ObjectsAndTiles;
        let (bytes, tiles) = fixture(mode);
        let first = tiles[0];
        let session = DecodeSession::new(DecodePolicy::default());
        let objects = ObjectReader::new(mode, context(), &session, 4096);
        let mut offset = first.offset as usize;
        let mut frames = Vec::new();
        loop {
            let o = objects.read(&bytes[offset..]).unwrap();
            frames.push((o.kind, o.body.to_vec()));
            offset += o.encoded_len;
            if o.kind == TILE_END {
                break;
            }
        }
        // Recompute local checksums after each structural/aggregate mutation.
        for mutation in 0..5 {
            let mut frames = frames.clone();
            match mutation {
                0 => frames[0].1[0] = 1,                   // wrong physical tile
                1 => frames.last_mut().unwrap().1[0] = 1,  // wrong end ID
                2 => frames.last_mut().unwrap().1[1] ^= 1, // byte count
                3 => frames.last_mut().unwrap().1[2] ^= 1, // child count
                4 => frames[1].1[0] ^= 1,                  // valid local digest, wrong aggregate
                _ => unreachable!(),
            }
            let mut changed = Vec::new();
            for (kind, body) in frames {
                append_object(&mut changed, kind, &body, mode, context(), 4096).unwrap();
            }
            let session = DecodeSession::new(DecodePolicy::default());
            let mut reader = TileReader::new(layout(), mode, context(), limits(), &session)
                .at_tile(0, first.offset)
                .unwrap();
            let mut offset = 0;
            while let Ok(event) = reader.read(&changed[offset..]) {
                offset += match event {
                    TileEvent::Begin { extent, .. } | TileEvent::Object { extent, .. } => {
                        extent.len as usize
                    }
                    TileEvent::End { .. } => panic!("accepted mutation {mutation}"),
                };
            }
            assert!(reader.read(&bytes[first.offset as usize..]).is_err());
        }
    }

    #[test]
    fn incomplete_nested_outside_and_oversize_tiles_are_rejected() {
        let (bytes, tiles) = fixture(ObjectIntegrity::None);
        let first = tiles[0];
        let range = &bytes[first.offset as usize..(first.offset + first.len) as usize];
        for max_tile_bytes in [0, first.len - 1] {
            let session = DecodeSession::new(DecodePolicy::default());
            let mut reader = TileReader::new(
                layout(),
                ObjectIntegrity::None,
                context(),
                TileLimits {
                    max_tile_bytes,
                    ..limits()
                },
                &session,
            );
            let mut offset = 0;
            while let Ok(event) = reader.read(&range[offset..]) {
                offset += match event {
                    TileEvent::Begin { extent, .. }
                    | TileEvent::Object { extent, .. }
                    | TileEvent::End { extent, .. } => extent.len as usize,
                };
            }
            assert!(reader.finish().is_err());
        }
        let session = DecodeSession::new(DecodePolicy::default());
        let mut reader = TileReader::new(
            layout(),
            ObjectIntegrity::None,
            context(),
            limits(),
            &session,
        );
        reader.read(range).unwrap();
        assert!(reader.read(range).is_err()); // nested begin
        assert!(reader.finish().is_err());
        let mut outside = Vec::new();
        append_object(
            &mut outside,
            53,
            b"unit",
            ObjectIntegrity::None,
            context(),
            4096,
        )
        .unwrap();
        let session = DecodeSession::new(DecodePolicy::default());
        let mut reader = TileReader::new(
            layout(),
            ObjectIntegrity::None,
            context(),
            limits(),
            &session,
        );
        assert!(reader.read(&outside).is_err());
        let session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
        let mut reader = TileReader::new(
            layout(),
            ObjectIntegrity::None,
            context(),
            limits(),
            &session,
        );
        assert!(matches!(reader.read(range), Err(Error::Cancelled)));
        assert_eq!(session.usage().input_bytes, 0);
        let mut w = TileWriter::new(layout(), ObjectIntegrity::None, context(), limits());
        w.begin(&[]).unwrap();
        w.consume_output(w.output().len()).unwrap();
        assert!(w.append(53, &vec![0; 4096]).is_err());
        assert!(w.output().is_empty());
    }
}
