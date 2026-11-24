//! Allocating object assembly shared by encoded-domain migrations.

use super::{
    DirectoryWriter, Entry, EntryKind, ObjectIntegrity, SetupContext, TileLayout, TileLimits,
    TileWriter,
};
use crate::Error;
use alloc::vec::Vec;

pub(crate) struct Converted {
    pub data: Vec<u8>,
    pub index: Vec<u8>,
    pub extra_features: crate::Features,
}

pub(crate) struct PayloadWriter {
    frames: TileWriter,
    directory: DirectoryWriter,
    data: Vec<u8>,
    entries: Vec<u8>,
    max_bytes: usize,
}

pub(crate) fn extend(output: &mut Vec<u8>, bytes: &[u8], max: usize) -> Result<(), Error> {
    if output
        .len()
        .checked_add(bytes.len())
        .ok_or(Error::TooLarge)?
        > max
    {
        return Err(Error::TooLarge);
    }
    output
        .try_reserve(bytes.len())
        .map_err(|_| Error::TooLarge)?;
    output.extend_from_slice(bytes);
    Ok(())
}

impl PayloadWriter {
    pub(crate) fn new(
        layout: TileLayout,
        mode: ObjectIntegrity,
        context: SetupContext,
        setup: &[u8],
        max_bytes: usize,
    ) -> Result<Self, Error> {
        let mut data = Vec::new();
        data.try_reserve_exact(setup.len().checked_add(11).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut data, setup.len() as u64);
        extend(&mut data, setup, max_bytes)?;
        crate::put_varint(&mut data, 1);
        Ok(Self {
            frames: TileWriter::new(
                layout,
                mode,
                context,
                TileLimits {
                    max_object_bytes: max_bytes as u64,
                    max_tile_bytes: max_bytes as u64,
                },
            ),
            directory: DirectoryWriter::new(layout, mode),
            data,
            entries: Vec::new(),
            max_bytes,
        })
    }
    #[cfg(feature = "encode")]
    pub(crate) fn with_checks(
        mut self,
        checks: alloc::sync::Arc<super::OwnedChecks>,
        group_kind: u64,
    ) -> Self {
        self.frames = self.frames.with_checks(Some(checks), group_kind);
        self
    }
    #[cfg(feature = "encode")]
    pub(crate) fn object_checked(
        &mut self,
        tag: u64,
        kind: EntryKind,
        role: u8,
        id: u64,
        body: &[u8],
        tags: &[u8],
    ) -> Result<(), Error> {
        self.frames.append_parts_checked(tag, &[body], tags)?;
        self.publish(kind, role, id)
    }
    #[cfg(feature = "encode")]
    pub(crate) fn end_checked(&mut self, id: u64, tags: &[u8]) -> Result<(), Error> {
        self.frames.end_checked(tags)?;
        self.publish(EntryKind::TileEnd, 0, id)
    }
    fn publish(&mut self, kind: EntryKind, role: u8, id: u64) -> Result<(), Error> {
        let entry = Entry {
            kind,
            role,
            id,
            record_len: self.frames.output().len() as u64,
        };
        let mut bytes = [0; 40];
        let n = self.directory.push(entry, &mut bytes)?;
        let total = self
            .data
            .len()
            .checked_add(self.frames.output().len())
            .and_then(|v| v.checked_add(self.entries.len()))
            .and_then(|v| v.checked_add(n))
            .ok_or(Error::TooLarge)?;
        if total > self.max_bytes {
            return Err(Error::TooLarge);
        }
        extend(&mut self.entries, &bytes[..n], self.max_bytes)?;
        extend(&mut self.data, self.frames.output(), self.max_bytes)?;
        self.frames.consume_output(self.frames.output().len())
    }
    pub(crate) fn object(
        &mut self,
        tag: u64,
        kind: EntryKind,
        role: u8,
        id: u64,
        body: &[u8],
    ) -> Result<(), Error> {
        self.frames.append(tag, body)?;
        self.publish(kind, role, id)
    }
    pub(crate) fn begin(&mut self, id: u64, plan: &[u8]) -> Result<(), Error> {
        self.frames.begin(plan)?;
        self.publish(EntryKind::TileBegin, 0, id)
    }
    pub(crate) fn end(&mut self, id: u64) -> Result<(), Error> {
        self.frames.end()?;
        self.publish(EntryKind::TileEnd, 0, id)
    }
    #[cfg(all(feature = "encode", feature = "std"))]
    pub(crate) fn take_output(&mut self) -> (Vec<u8>, Vec<u8>) {
        (
            core::mem::take(&mut self.data),
            core::mem::take(&mut self.entries),
        )
    }
    #[cfg(all(feature = "encode", feature = "std"))]
    pub(crate) fn restore_output(&mut self, mut data: Vec<u8>, mut entries: Vec<u8>) {
        data.clear();
        entries.clear();
        self.data = data;
        self.entries = entries;
    }
    #[cfg(all(feature = "encode", feature = "std"))]
    pub(crate) fn complete(&self) -> Result<(u64, u64), Error> {
        self.frames.finish()?;
        self.directory.finish()
    }
    pub(crate) fn finish(self) -> Result<Converted, Error> {
        self.frames.finish()?;
        let (count, objects_len) = self.directory.finish()?;
        let mut index = Vec::new();
        index
            .try_reserve_exact(self.entries.len().checked_add(30).ok_or(Error::TooLarge)?)
            .map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut index, count);
        extend(&mut index, &self.entries, self.max_bytes)?;
        index.extend_from_slice(&objects_len.to_le_bytes());
        let payload = index.len().checked_add(12).ok_or(Error::TooLarge)?;
        let chunk = 4usize
            .checked_add(crate::varint_len(payload as u64 + 1))
            .and_then(|n| n.checked_add(payload))
            .ok_or(Error::TooLarge)?;
        index.extend_from_slice(&(chunk as u64).to_le_bytes());
        index.extend_from_slice(b"uIDX");
        if self
            .data
            .len()
            .checked_add(index.len())
            .ok_or(Error::TooLarge)?
            > self.max_bytes
        {
            return Err(Error::TooLarge);
        }
        Ok(Converted {
            data: self.data,
            index,
            extra_features: crate::Features::EMPTY,
        })
    }
}
