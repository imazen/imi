//! Parsed frame state stores spans, so owning the input never borrows itself.
use super::*;
use alloc::borrow::Cow;
use core::ops::Range;

pub(super) fn span(source: &[u8], bytes: &[u8]) -> Result<Range<usize>, Error> {
    let start = (bytes.as_ptr() as usize)
        .checked_sub(source.as_ptr() as usize)
        .ok_or(Error::Corrupt("frame span outside input"))?;
    let end = start.checked_add(bytes.len()).ok_or(Error::TooLarge)?;
    if end > source.len() {
        return Err(Error::Corrupt("frame span outside input"));
    }
    Ok(start..end)
}

pub(super) enum PixelTable {
    Absent,
    Unknown,
    Known(Range<usize>, crate::HashAlgorithm),
}
impl PixelTable {
    pub(super) fn new(source: &[u8], table: crate::pixelsum::Table<'_>) -> Result<Self, Error> {
        Ok(match table {
            crate::pixelsum::Table::Absent => Self::Absent,
            crate::pixelsum::Table::Unknown => Self::Unknown,
            crate::pixelsum::Table::Known(bytes, algo) => Self::Known(span(source, bytes)?, algo),
        })
    }
    pub(super) fn view<'a>(&self, source: &'a [u8]) -> crate::pixelsum::Table<'a> {
        match self {
            Self::Absent => crate::pixelsum::Table::Absent,
            Self::Unknown => crate::pixelsum::Table::Unknown,
            Self::Known(range, algo) => {
                crate::pixelsum::Table::Known(&source[range.clone()], *algo)
            }
        }
    }
}

/// Only the validated image fields consumed by frame reconstruction. Metadata
/// is already retained by FrameDecoder; file checksums were handled at parsing.
pub(super) struct ImageRecord {
    pub(super) header: crate::Header,
    declared: Features,
    data: Range<usize>,
    data_open: bool,
    encoded_len: usize,
    index: Option<(Range<usize>, u64)>,
    pixelsum: PixelTable,
}
impl ImageRecord {
    pub(super) fn new(source: &[u8], file: container::File<'_>) -> Result<Self, Error> {
        Ok(Self {
            header: file.header,
            declared: file.declared,
            data: span(source, file.data)?,
            data_open: file.data_open,
            encoded_len: file.encoded_len,
            index: file
                .trailing_index
                .map(|(body, len)| Ok((span(source, body)?, len)))
                .transpose()?,
            pixelsum: PixelTable::new(source, file.pixelsum)?,
        })
    }
    pub(super) fn view<'a>(&self, source: &'a [u8]) -> container::File<'a> {
        container::File {
            encoded_len: self.encoded_len,
            header: self.header.clone(),
            declared: self.declared,
            data: &source[self.data.clone()],
            data_open: self.data_open,
            pixelsum: self.pixelsum.view(source),
            metadata: Vec::new(),
            previews: Vec::new(),
            anim: None,
            frames: Vec::new(),
            checksum: None,
            trailing_index: self
                .index
                .as_ref()
                .map(|(span, len)| (&source[span.clone()], *len)),
        }
    }
}

impl FrameDecoder<'static> {
    /// Own complete compressed input while retaining the same staged admission,
    /// metadata, frame controls and rendering as [`Self::with_limits`]. No input
    /// copy or decoded-frame cache is created; input spans remain valid when this
    /// decoder is moved between threads. Each nested item is parsed when reached.
    pub fn from_owned(data: Vec<u8>, limits: &crate::Limits) -> Result<Self, Error> {
        Self::from_owned_admitted(data, limits, &crate::parse::limits::Unrestricted)
    }

    pub(crate) fn from_owned_admitted(
        data: Vec<u8>,
        limits: &crate::Limits,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        let decoder =
            FrameDecoder::open_admitted(&data, limits.verify_checksum, Some(*limits), admission)?;
        let FrameDecoder {
            data: _,
            canvas,
            meta,
            defaults,
            kind,
            pixelsum,
            pixel_status,
            loop_count,
            first_duration,
            frames,
            declared,
            version,
            nested,
            outer,
            pos,
            cur,
            cur_page,
            unified,
            reference,
        } = decoder;
        Ok(FrameDecoder {
            data: Cow::Owned(data),
            canvas,
            meta,
            defaults,
            kind,
            pixelsum,
            pixel_status,
            loop_count,
            first_duration,
            frames,
            declared,
            version,
            nested,
            outer,
            pos,
            cur,
            cur_page,
            unified,
            reference,
        })
    }
}
