//! Resumable state for the same admitted native row kernels.

use super::*;

// Fixed-size state, like Prepared: no allocation proportional to image height.
#[allow(clippy::large_enum_variant)]
enum RowStream<'a> {
    V4(alloc::boxed::Box<crate::v4::plan::Rows<'a>>),
    #[cfg(feature = "native-jpeg")]
    Native(crate::jpeg::wire::NativeStream<'a>),
    Lossless(crate::v1::LosslessStream<'a>),
    Lossy(crate::lossy::LossyStream<'a>),
    #[cfg(feature = "native-jpeg")]
    OwnedNative(crate::jpeg::wire::OwnedNative),
    OwnedLossless(crate::v1::OwnedLossless),
    OwnedLossy(crate::lossy::LossyStream<'static, crate::lossy::OwnedRecords>),
}

pub(crate) struct AdmittedRows<'a> {
    stream: RowStream<'a>,
    info: ImageInfo,
    checksum: Option<crate::pixelsum::RowCheck>,
    status: crate::PixelChecksumStatus,
    y: u32,
    failed: Option<Error>,
}

impl<C: Fn() -> bool + Sync> DecodeSession<C> {
    pub(crate) fn owned_row_state(
        &mut self,
        data: alloc::vec::Vec<u8>,
    ) -> Result<AdmittedRows<'static>, Error> {
        self.begin(&data)?;
        if crate::parse::header::header_version(&data)? == 4 {
            return self.v4_row_state(alloc::borrow::Cow::Owned(data));
        }
        let admission = RequestAdmission {
            session: self,
            profile: ExecutionProfile::Rows,
        };
        let (parsed, table) = prepare(&data, &admission, self.policy.output.verify_checksum)?;
        let extra = match &parsed {
            #[cfg(feature = "native-jpeg")]
            Prepared::Native(p) => {
                p.bands.len() as u128 * core::mem::size_of::<core::ops::Range<usize>>() as u128
            }
            Prepared::Lossless(p) => crate::v1::owned_extra_bytes(p),
            Prepared::Lossy(p) => crate::lossy::OwnedRecords::extra_bytes(p),
        };
        // During conversion both borrowed record vectors and offset vectors
        // exist. Charge that peak before allocating, in addition to the common
        // decoded row/stripe plan. Compressed input ownership is transferred,
        // not copied; its capacity is covered by the adapter's input reservation.
        self.memory(core::mem::size_of::<AdmittedRows<'static>>() as u128 + extra)?;
        self.work(extra)?;
        let info = parsed.info().clone();
        let checksum = crate::pixelsum::RowCheck::new(table, &info)?;
        let status = table.status();
        self.check()?;
        let stream = match parsed {
            #[cfg(feature = "native-jpeg")]
            Prepared::Native(p) => {
                let spans = crate::owned::spans(&data, &p.bands, &|| self.check())?;
                let crate::jpeg::wire::NativeParsed {
                    head, payload_len, ..
                } = p;
                RowStream::OwnedNative(crate::jpeg::wire::OwnedNative::new(
                    data,
                    head,
                    spans,
                    payload_len,
                )?)
            }
            Prepared::Lossless(p) => {
                let spans = crate::owned::spans(&data, &p.stripes, &|| self.check())?;
                let p = p.without_records();
                RowStream::OwnedLossless(crate::v1::OwnedLossless::new(data, p, spans))
            }
            Prepared::Lossy(p) => {
                let spans = crate::lossy::OwnedRecords::spans(&data, &p, &|| self.check())?;
                let p = p.without_records();
                RowStream::OwnedLossy(crate::lossy::LossyStream::from_owned(data, p, spans)?)
            }
        };
        self.check()?;
        Ok(AdmittedRows {
            stream,
            info,
            checksum,
            status,
            y: 0,
            failed: None,
        })
    }

    pub(super) fn v4_row_state<'a>(
        &self,
        data: alloc::borrow::Cow<'a, [u8]>,
    ) -> Result<AdmittedRows<'a>, Error> {
        let _input = self.v4_precharged()?;
        let (rows, checksum, status) = crate::v4::plan::Rows::new(data, self)?;
        let info = rows.info().clone();
        Ok(AdmittedRows {
            stream: RowStream::V4(alloc::boxed::Box::new(rows)),
            info,
            checksum,
            status,
            y: 0,
            failed: None,
        })
    }

    pub(crate) fn row_state<'a>(&mut self, data: &'a [u8]) -> Result<AdmittedRows<'a>, Error> {
        self.begin(data)?;
        if crate::parse::header::header_version(data)? == 4 {
            return self.v4_row_state(alloc::borrow::Cow::Borrowed(data));
        }
        let admission = RequestAdmission {
            session: self,
            profile: ExecutionProfile::Rows,
        };
        let (parsed, table) = prepare(data, &admission, self.policy.output.verify_checksum)?;
        self.memory(core::mem::size_of::<AdmittedRows<'_>>() as u128)?;
        let info = parsed.info().clone();
        let checksum = crate::pixelsum::RowCheck::new(table, &info)?;
        self.check()?;
        let stream = match parsed {
            #[cfg(feature = "native-jpeg")]
            Prepared::Native(p) => RowStream::Native(crate::jpeg::wire::NativeStream::new(p)?),
            Prepared::Lossless(p) => RowStream::Lossless(crate::v1::LosslessStream::from_parsed(p)),
            Prepared::Lossy(p) => RowStream::Lossy(crate::lossy::LossyStream::from_parsed(p)?),
        };
        self.check()?;
        Ok(AdmittedRows {
            stream,
            info,
            checksum,
            status: table.status(),
            y: 0,
            failed: None,
        })
    }
}

impl AdmittedRows<'_> {
    pub(crate) fn info(&self) -> &ImageInfo {
        &self.info
    }
    pub(crate) fn usage(&self) -> Option<DecodeUsage> {
        match &self.stream {
            RowStream::V4(rows) => Some(rows.usage()),
            _ => None,
        }
    }
    pub(crate) fn rows_decoded(&self) -> u32 {
        self.y
    }
    pub(crate) fn status(&self) -> crate::PixelChecksumStatus {
        self.checksum.as_ref().map_or(self.status, |c| c.status())
    }
    pub(crate) fn next(
        &mut self,
        row: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self.next_inner(row, check);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    fn next_inner(
        &mut self,
        row: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        check()?;
        if self.y == self.info.height {
            return Ok(false);
        }
        let bytes = (self.info.width as usize)
            .checked_mul(self.info.bytes_per_pixel())
            .ok_or(Error::TooLarge)?;
        let row = row
            .get_mut(..bytes)
            .ok_or(Error::InvalidInput("row buffer too small"))?;
        let produced = match &mut self.stream {
            RowStream::V4(rows) => rows.next_row_checked(row, check),
            #[cfg(feature = "native-jpeg")]
            RowStream::Native(s) => s.next_row_checked(row, check),
            RowStream::Lossless(s) => s.next_row_checked(row, check),
            RowStream::Lossy(s) => s.next_row_checked(row, check),
            #[cfg(feature = "native-jpeg")]
            RowStream::OwnedNative(s) => s.next_row_checked(row, check),
            RowStream::OwnedLossless(s) => s.next_row_checked(row, check),
            RowStream::OwnedLossy(s) => s.next_row_checked(row, check),
        }?;
        if !produced {
            return Err(Error::Corrupt("primary image too short"));
        }
        check()?;
        if let Some(checksum) = &mut self.checksum {
            checksum.row_checked(row, check)?;
        }
        check()?;
        self.y += 1;
        Ok(true)
    }
}
