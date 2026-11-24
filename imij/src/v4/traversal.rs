//! Positional structural validation: one complete object, never a raster/file.
use super::{Directory, DirectoryPlacement, LocatedEntry, ObjectIntegrity, TileLayout, TileLimits};
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::parse::payload::HeadInput;
use crate::traversal::{Cursor, ScanLimits};
use crate::{ByteReader, Error};
use alloc::vec::Vec;

pub(crate) struct Objects {
    entries: Vec<LocatedEntry>,
    layout: TileLayout,
    integrity: ObjectIntegrity,
    front: Option<Vec<u8>>,
    placement: DirectoryPlacement,
    len: u64,
    _memory: crate::decode_resources::V4Retained,
}
// Retain bounded parser state inline; boxing would add an allocation per scan.
#[allow(clippy::large_enum_variant)]
enum Driver<'s> {
    Numeric(super::NumericGroupDecoder<'s, fn() -> bool>),
    Lossy(super::LossyGroupDecoder<'s, fn() -> bool>),
}
impl Driver<'_> {
    fn read(&mut self, bytes: &[u8]) -> Result<(LocatedEntry, bool), Error> {
        match self {
            Self::Numeric(d) => {
                let e = d.read(bytes)?;
                Ok((e.entry(), matches!(e, super::NumericEvent::End { .. })))
            }
            Self::Lossy(d) => {
                let e = d.read(bytes)?;
                Ok((e.entry(), matches!(e, super::LossyEvent::End { .. })))
            }
        }
    }
    fn finish(&self) -> Result<(), Error> {
        match self {
            Self::Numeric(d) => d.finish_structure(),
            Self::Lossy(d) => d.finish_structure(),
        }
    }
}
impl Objects {
    pub(crate) fn scan(
        r: &mut Cursor<'_>,
        hp: &crate::parse::header::ParsedHeader,
        bounded_end: Option<u64>,
        limits: ScanLimits,
        session: &DecodeSession,
    ) -> Result<(Self, u64, Vec<(u64, u64)>, Vec<u64>), Error> {
        let memory = session.v4_model_scope();
        let start = r.pos;
        let length = r.varint()?;
        if length > limits.head as u64 {
            return Err(Error::TooLarge);
        }
        let end = r.pos.checked_add(length).ok_or(Error::TooLarge)?;
        r.skip_to(end)?;
        let placement = DirectoryPlacement::try_from(r.varint()?)?;
        if placement == DirectoryPlacement::Front {
            let capacity = r.varint()?;
            if capacity > limits.metadata as u64 {
                return Err(Error::TooLarge);
            }
            r.skip_to(r.pos.checked_add(capacity).ok_or(Error::TooLarge)?)?;
        }
        let objects_at = r.pos;
        let prefix_len = usize::try_from(objects_at - start).map_err(|_| Error::TooLarge)?;
        let prefix_memory = session.v4_working(prefix_len as u128)?;
        r.pos = start;
        let prefix = r.vector(prefix_len as u64, prefix_len)?;
        let tile_limits = TileLimits {
            max_object_bytes: u64::MAX,
            max_tile_bytes: u64::MAX,
        };
        let (mut driver, layout, integrity) = match hp.header.coding {
            crate::Coding::Lossless => {
                let (d, n) = super::NumericGroupDecoder::new(
                    hp.header.info.clone(),
                    hp.declared,
                    &prefix,
                    tile_limits,
                    session,
                )?;
                if n != prefix.len() {
                    return Err(Error::Corrupt("V4 structural prefix"));
                }
                if let Some(checks) = d.check_config() {
                    checks.get().require_supported()?;
                }
                let (layout, integrity) = (d.layout(), d.integrity());
                (Driver::Numeric(d), layout, integrity)
            }
            crate::Coding::Lossy => {
                let (d, n) = super::LossyGroupDecoder::new(
                    hp.header.info.clone(),
                    hp.header
                        .lossy_color
                        .ok_or(Error::Corrupt("V4 lossy color"))?,
                    hp.declared,
                    &prefix,
                    tile_limits,
                    session,
                )?;
                if n != prefix.len() {
                    return Err(Error::Corrupt("V4 structural prefix"));
                }
                d.setup().checks().require_supported()?;
                let (layout, integrity) = (d.setup().layout(), d.setup().integrity());
                (Driver::Lossy(d), layout, integrity)
            }
        };
        let mut p = ByteReader::new(&prefix);
        let n = p.var_usize()?;
        p.bytes(n)?;
        let storage = super::DirectoryPrefix::read(&prefix[p.pos..], session)?;
        let front = storage
            .front
            .map(|b| {
                session.v4_models().hint(b.len())?;
                let mut out = Vec::new();
                out.try_reserve_exact(b.len())
                    .map_err(|_| Error::TooLarge)?;
                out.extend_from_slice(b);
                Ok::<_, Error>(out)
            })
            .transpose()?;
        let mut entries = Vec::new();
        let mut units = Vec::new();
        let mut spans = Vec::new();
        let mut tiles = 0;
        while bounded_end.map_or(tiles < layout.tile_count(), |end| r.pos < end) {
            if entries.len() >= limits.units.saturating_mul(2) {
                return Err(Error::TooLarge);
            }
            let at = r.pos;
            let kind = r.varint()?;
            let length = r.varint()?;
            let end = r.pos.checked_add(length).ok_or(Error::TooLarge)?;
            if bounded_end.is_some_and(|bound| end > bound) {
                return Err(Error::Corrupt("V4 object past DATA"));
            }
            let length = usize::try_from(end - at).map_err(|_| Error::TooLarge)?;
            let _object_memory = session.v4_working(length as u128)?;
            r.pos = at;
            let bytes = r.vector(length as u64, length)?;
            let (entry, closed) = driver.read(&bytes)?;
            if entry.offset != at - objects_at || entry.entry.record_len != length as u64 {
                return Err(Error::Corrupt("V4 structural object extent"));
            }
            tiles += u64::from(closed);
            session.adapter_work(core::mem::size_of::<LocatedEntry>() as u128 + 24)?;
            session.v4_reserve(&mut entries, 1)?;
            session.v4_reserve(&mut units, 1)?;
            session.v4_reserve(&mut spans, 1)?;
            entries.push(entry);
            units.push((at, kind));
            spans.push(end - objects_at);
        }
        driver.finish()?;
        // The prefix is temporary. Commit only buffers kept by the scan.
        drop(prefix);
        drop(prefix_memory);
        Ok((
            Self {
                entries,
                layout,
                integrity,
                front,
                placement,
                len: r.pos - objects_at,
                _memory: memory.commit()?,
            },
            objects_at - start,
            units,
            spans,
        ))
    }
    pub(crate) fn check(
        &self,
        tail: Option<(&[u8], u64)>,
        session: &DecodeSession,
    ) -> Result<(), Error> {
        let index = match self.placement {
            DirectoryPlacement::None => {
                if tail.is_some() {
                    return Err(Error::Corrupt("V4 undeclared directory"));
                }
                None
            }
            DirectoryPlacement::Front => {
                if tail.is_some() {
                    return Err(Error::Corrupt("competing V4 directories"));
                }
                Some(
                    self.front
                        .as_deref()
                        .ok_or(Error::Corrupt("unfinished V4 front directory"))?,
                )
            }
            DirectoryPlacement::Tail => {
                let (body, len) = tail.ok_or(Error::Corrupt("missing V4 tail directory"))?;
                Some(super::file::check_tail_locator(body, len)?)
            }
        };
        if let Some(index) = index {
            let d = Directory::read(index, self.layout, self.integrity, self.len, session)?;
            let mut at = 0;
            d.visit(session, |entry, _| {
                if self.entries.get(at) != Some(&entry) {
                    return Err(Error::Corrupt("V4 physical/directory mismatch"));
                }
                at += 1;
                Ok(true)
            })?;
            if at != self.entries.len() {
                return Err(Error::Corrupt("V4 physical/directory count"));
            }
        }
        Ok(())
    }
}
