//! Native band byte spans for archive updates that copy unchanged coefficients.

use super::{NativeHeader, wire};
use crate::parse::records::UnitMap;
use crate::{ByteReader, Error};
use alloc::vec::Vec;
use core::ops::Range;

/// Validated native band index over caller-owned file bytes. Opening verifies
/// file checksums, geometry, unit framing/index and carrier declarations;
/// entropy and pixels are checked when decoding. Spans include an optional
/// side record and the complete main record, suitable for range-copy storage.
pub struct NativeArchive<'a> {
    pub(crate) header: NativeHeader,
    /// The file's header version: its band records are numbered for it.
    version: u8,
    pub(crate) rows: u16,
    file: &'a [u8],
    spans: Vec<Range<usize>>,
}

impl<'a> NativeArchive<'a> {
    pub fn open(file: &'a [u8]) -> Result<Self, Error> {
        let f = crate::v1::parse_file(
            file,
            &crate::parse::limits::Unrestricted,
            crate::checksum::Verify::Now(1),
        )?;
        if !crate::parse::payload::native_selected(f.header.lossy_color, f.declared, f.data)? {
            return Err(Error::InvalidInput("file has no native bands"));
        }
        let p = wire::parse(
            f.header.info.clone(),
            f.header.version,
            f.data,
            &crate::parse::limits::Unrestricted,
        )?;
        if p.payload_len != f.data.len() {
            return Err(Error::Corrupt("trailing DATA bytes"));
        }
        let mut r = ByteReader::new(f.data);
        let mut head = wire::read_head(f.header.info, &mut r, &crate::parse::limits::Structure)?;
        let count = head.bands();
        let lengths = UnitMap::native().spans(&mut r, f.header.version, count, head.hint.take())?;
        let mut spans = Vec::new();
        spans
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        let mut at = f.data.as_ptr() as usize - file.as_ptr() as usize + p.payload_len
            - lengths.iter().map(|&n| n as usize).sum::<usize>();
        for n in lengths {
            let end = at
                .checked_add(usize::try_from(n).map_err(|_| Error::TooLarge)?)
                .ok_or(Error::TooLarge)?;
            spans.push(at..end);
            at = end;
        }
        Ok(Self {
            header: head.header,
            version: f.header.version,
            rows: head.rows,
            file,
            spans,
        })
    }
    pub fn header(&self) -> &NativeHeader {
        &self.header
    }
    /// The archive file's header version (SPEC 1.0), which numbers its band
    /// records; `super::BandEncoder::copy_band_record` needs it.
    pub fn header_version(&self) -> u8 {
        self.version
    }
    pub fn mcu_rows_per_band(&self) -> u16 {
        self.rows
    }
    pub fn band_count(&self) -> usize {
        self.spans.len()
    }
    /// Absolute file byte range, including its side record when present.
    pub fn band_span(&self, band: usize) -> Result<Range<usize>, Error> {
        self.spans
            .get(band)
            .cloned()
            .ok_or(Error::InvalidInput("native archive band index"))
    }
    pub fn band_record(&self, band: usize) -> Result<&'a [u8], Error> {
        Ok(&self.file[self.band_span(band)?])
    }
}
