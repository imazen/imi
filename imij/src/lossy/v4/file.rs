//! Complete lossy V4 files over the shared object/dependency drivers.

use super::assemble::Assembler;
use super::{LossyGroupDecoder, LossyWork};
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::v4::{Directory, LocatedEntry, TileLimits};
use crate::{Error, Header, ImageRef, Metadata};

/// Open validates declarations, directory and supported container checksums.
/// Successful decode additionally reconciles every physical object and pixel
/// checksum. Returned groups before finish have only their completed scopes.
pub struct LossyFile<'a, 's, C> {
    file: crate::container::File<'a>,
    decoder: LossyGroupDecoder<'s, C>,
    objects: &'a [u8],
    directory: Option<Directory<'a>>,
    session: &'s DecodeSession<C>,
}

impl<'a, 's, C: Fn() -> bool + Sync> LossyFile<'a, 's, C> {
    pub fn open(
        data: &'a [u8],
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<Self, Error> {
        let file = crate::v4::file::parse_file(data, session)?;
        Self::from_parsed(file, data.len(), limits, session)
    }

    pub(crate) fn from_parsed(
        file: crate::container::File<'a>,
        file_length: usize,
        limits: TileLimits,
        session: &'s DecodeSession<C>,
    ) -> Result<Self, Error> {
        Self::from_inherited(file, file_length, limits, session, None)
    }

    pub(crate) fn from_inherited(
        file: crate::container::File<'a>,
        file_length: usize,
        limits: TileLimits,
        session: &'s DecodeSession<C>,
        inherited: Option<&crate::ImageInfo>,
    ) -> Result<Self, Error> {
        if file.header.coding != crate::Coding::Lossy {
            return Err(Error::Unsupported("V4 lossy file requires lossy coding"));
        }
        let (decoder, used, storage) = LossyGroupDecoder::with_inherited_directory(
            file.header.info.clone(),
            file.header
                .lossy_color
                .ok_or(Error::Corrupt("V4 lossy color missing"))?,
            file.declared,
            file.data,
            limits,
            session,
            inherited,
        )?;
        let (objects, directory) = crate::v4::file::read_directory(
            file_length,
            &file,
            used,
            decoder.setup().layout(),
            decoder.setup().integrity(),
            storage,
            session,
        )?;
        Ok(Self {
            file,
            decoder,
            objects,
            directory,
            session,
        })
    }

    #[cfg(feature = "lab")]
    pub(super) fn inspection_setup(&self) -> &super::LossySetup {
        self.decoder.setup()
    }
    #[cfg(feature = "lab")]
    pub(super) fn inspection_objects(&self) -> &[u8] {
        self.objects
    }

    pub fn header(&self) -> &Header {
        &self.file.header
    }
    pub fn metadata(&self) -> Result<Metadata, Error> {
        self.file.metadata()
    }
    pub(crate) fn check_config(&self) -> Option<&alloc::sync::Arc<crate::v4::OwnedChecks>> {
        self.decoder.setup().checks.as_ref()
    }

    pub(crate) fn pixelsum(&self) -> crate::pixelsum::Table<'a> {
        self.file.pixelsum
    }
    pub(crate) fn layout(&self) -> crate::v4::TileLayout {
        self.decoder.setup().layout()
    }
    pub(crate) fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        self.decoder.setup().group_rect(id)
    }

    /// Deliver independent AC work in physical order, before tile/file close.
    /// Callback input/dependency queues remain bounded by their owners. Finish
    /// verifies full physical structure, but not an unconstructed bitmap hash.
    pub fn visit_groups(
        self,
        mut group: impl FnMut(LossyWork<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        let mut assembler = Assembler::new(self.decoder.shared_setup(), self.session);
        self.visit_units(|event| {
            if let Some(work) = assembler.event(event)? {
                group(work)?;
            }
            Ok(())
        })
    }

    pub(crate) fn visit_checked(
        self,
        mut visit: impl FnMut(crate::v4::decode::ImageEvent<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        let setup = self.decoder.shared_setup();
        let verify_tile = self
            .session
            .v4_pixel_checks(crate::v4::CheckScope::TilePixels);
        let mut assembler = Assembler::new(setup.clone(), self.session);
        self.visit_units(|event| {
            if let super::LossyEvent::End {
                tile,
                pixel_results,
                ..
            } = &event
                && verify_tile
                && let Some(checks) = &setup.checks
                && checks.width(crate::v4::CheckScope::TilePixels) != 0
            {
                visit(crate::v4::decode::ImageEvent::Tile(
                    crate::v4::TilePixelCheck {
                        checks: checks.clone(),
                        id: tile.tile_id,
                        rect: crate::v4::tile_pixel_rect(setup.layout(), tile.tile_id, |id| {
                            setup.group_rect(id)
                        })?,
                        expected: pixel_results,
                    },
                ))?;
            }
            if let Some(work) = assembler.event(event)? {
                visit(crate::v4::decode::ImageEvent::Group(
                    crate::v4::decode::GroupWork::Lossy(work),
                ))?;
            }
            Ok(())
        })
    }

    pub(crate) fn visit_units(
        mut self,
        mut unit: impl FnMut(super::LossyEvent<'a>) -> Result<(), Error>,
    ) -> Result<(Header, Metadata), Error> {
        let metadata = self.file.metadata()?;
        let mut offset = 0usize;
        let mut consume = |at: usize, expected: Option<LocatedEntry>| -> Result<usize, Error> {
            let available = if let Some(entry) = expected {
                if entry.offset != at as u64 {
                    return Err(Error::Corrupt("V4 lossy physical/index offset"));
                }
                let end = at
                    .checked_add(
                        usize::try_from(entry.entry.record_len).map_err(|_| Error::TooLarge)?,
                    )
                    .ok_or(Error::TooLarge)?;
                self.objects
                    .get(at..end)
                    .ok_or(Error::Corrupt("V4 indexed lossy bounds"))?
            } else {
                &self.objects[at..]
            };
            let event = self.decoder.read(available)?;
            let observed = event.entry();
            if expected.is_some_and(|entry| entry != observed) {
                return Err(Error::Corrupt("V4 lossy physical/directory mismatch"));
            }
            let end = at
                .checked_add(
                    usize::try_from(observed.entry.record_len).map_err(|_| Error::TooLarge)?,
                )
                .ok_or(Error::TooLarge)?;
            unit(event)?;
            Ok(end)
        };
        if let Some(directory) = &self.directory {
            directory.visit(self.session, |entry, _| {
                offset = consume(offset, Some(entry))?;
                Ok(true)
            })?;
        } else {
            while offset < self.objects.len() {
                offset = consume(offset, None)?;
            }
        }
        if offset != self.objects.len() {
            return Err(Error::Corrupt("V4 trailing lossy bytes"));
        }
        self.decoder.finish_structure()?;
        Ok((self.file.header, metadata))
    }

    pub fn decode_into(
        self,
        output: &mut [u8],
        stride: usize,
    ) -> Result<(Header, Metadata), Error> {
        self.decode_with_reference(output, stride, None)
    }

    pub fn decode_with_reference(
        self,
        output: &mut [u8],
        stride: usize,
        reference: Option<ImageRef<'_>>,
    ) -> Result<(Header, Metadata), Error> {
        let info = self.file.header.info.clone();
        self.session.v4_output(&info)?;
        let row = (info.width as usize)
            .checked_mul(info.bytes_per_pixel())
            .ok_or(Error::TooLarge)?;
        let extent = (info.height as usize - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::TooLarge)?;
        if stride < row || output.len() < extent {
            return Err(Error::InvalidInput("V4 output stride/extent"));
        }
        if let Some(source) = reference {
            let n = (info.height as usize - 1)
                .checked_mul(source.stride)
                .and_then(|n| n.checked_add(row))
                .ok_or(Error::TooLarge)?;
            if source.info != &info || source.stride < row || source.pixels.len() < n {
                return Err(Error::InvalidInput("V4 reference interpretation/extent"));
            }
        }
        let pixelsum = self.file.pixelsum;
        let session = self.session;
        if matches!(pixelsum, crate::pixelsum::Table::Known(..)) {
            session.admission().pixel_checksum(&info)?;
        }
        let result = self.visit_checked(|event| {
            let work = match event {
                crate::v4::decode::ImageEvent::Group(crate::v4::decode::GroupWork::Lossy(work)) => {
                    work
                }
                crate::v4::decode::ImageEvent::Group(_) => {
                    return Err(Error::Corrupt("V4 lossy work kind"));
                }
                crate::v4::decode::ImageEvent::Tile(tile) => {
                    tile.verify(&info, output, stride, session)?;
                    return Ok(());
                }
            };
            let rect = work.rect()?;
            let at = (rect.y() as usize)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(rect.x() as usize * info.bytes_per_pixel()))
                .ok_or(Error::TooLarge)?;
            let mut local = info.clone();
            local.width = rect.width();
            local.height = rect.height();
            let reference = reference.map(|source| {
                ImageRef::new(
                    &source.pixels[rect.y() as usize * source.stride
                        + rect.x() as usize * info.bytes_per_pixel()..],
                    source.stride,
                    &local,
                )
            });
            work.decode_with_reference(&mut output[at..], stride, reference, session)?;
            Ok(())
        })?;
        pixelsum.verify_checked(0, &info, output, stride, || session.adapter_work(64 * 1024))?;
        Ok(result)
    }
}
