//! Borrowed or owned compressed input and one resumable decoded row.

use super::*;

/// Resumable zencodec row pull from complete compressed input.
///
/// The constructor admits and parses the file, without reconstructing its rows.
/// `StreamingDecode::next_batch` advances one native row. The compressed slice
/// must stay alive for `row_decoder`; `owned_row_decoder` takes ownership.
/// Output storage is one row plus native stripe/group state.
/// This does not accept incremental compressed input. Failures are sticky.
pub struct RowDecoder<'a> {
    job: DecodeRequest,
    state: crate::decode_resources::AdmittedRows<'a>,
    pixels: PixelBuffer,
    info: zencodec::ImageInfo,
    native: DecodedMetadata,
    usage: crate::decode_resources::DecodeUsage,
}

impl DecodeRequest {
    /// Pull one decoded row at a time using the common `StreamingDecode` trait.
    ///
    /// The complete borrowed input is admitted before output; file checksums
    /// follow the job policy. Pixel checksum coverage becomes verified after the
    /// last row. The existing `DecodeJob::streaming_decoder` factory remains the
    /// explicitly buffered owned-output alternative.
    pub fn row_decoder<'a>(
        self,
        data: &'a [u8],
        preferred: &[PixelDescriptor],
    ) -> Result<RowDecoder<'a>, DecodeError> {
        self.rows_from(Cow::Borrowed(data), preferred)
    }

    /// Own complete compressed input and pull one row at a time.
    ///
    /// Transfers the Vec without copying it or retaining a decoded raster.
    /// Its actual capacity is included in the job's memory admission. Record
    /// offsets and row/stripe state are also charged. This is not incremental
    /// compressed input; parsing and file checksum admission precede output.
    /// The returned decoder has no borrowed input lifetime.
    pub fn owned_row_decoder(
        self,
        data: Vec<u8>,
        preferred: &[PixelDescriptor],
    ) -> Result<RowDecoder<'static>, DecodeError> {
        self.rows_from(Cow::Owned(data), preferred)
    }

    fn rows_from<'a>(
        self,
        data: Cow<'a, [u8]>,
        preferred: &[PixelDescriptor],
    ) -> Result<RowDecoder<'a>, DecodeError> {
        let input_capacity = match &data {
            Cow::Borrowed(_) => 0,
            Cow::Owned(v) => v.capacity() as u64,
        };
        self.limits
            .check_memory(input_capacity)
            .map_err(|_| Error::TooLarge)?;
        let d = self.describe_reserved(&data, preferred, input_capacity)?;
        let row_bytes = d
            .descriptor
            .aligned_stride(d.native.width)
            .checked_add(d.descriptor.min_alignment() - 1)
            .ok_or(Error::TooLarge)?;
        let reserved = d
            .retained
            .checked_add(row_bytes as u64)
            .and_then(|v| v.checked_add(input_capacity))
            .ok_or(Error::TooLarge)?;
        let mut session = self.session(reserved)?;
        let state = match data {
            Cow::Borrowed(data) => session.row_state(data)?,
            Cow::Owned(data) => session.owned_row_state(data)?,
        };
        let usage = session.usage();
        drop(session);
        self.check()?;
        let pixels = PixelBuffer::try_new(d.native.width, 1, d.descriptor)
            .map_err(|_| Error::TooLarge)?
            .with_color_context(Arc::new(d.info.source_color.to_color_context()));
        Ok(RowDecoder {
            job: self,
            state,
            pixels,
            info: d.info,
            native: DecodedMetadata {
                image: d.native,
                metadata: d.metadata,
            },
            usage,
        })
    }
}

impl RowDecoder<'_> {
    /// Exact native description and policy-filtered metadata.
    pub fn native_metadata(&self) -> &DecodedMetadata {
        &self.native
    }
    /// Full-image admission charges, including work reserved before the first row.
    pub fn usage(&self) -> crate::decode_resources::DecodeUsage {
        self.state.usage().unwrap_or(self.usage)
    }
    /// Successfully delivered rows. A failed row is never counted.
    pub fn rows_decoded(&self) -> u32 {
        self.state.rows_decoded()
    }
    /// Primary-image decoded bitmap verification progress.
    pub fn pixel_checksum_status(&self) -> crate::PixelChecksumStatus {
        self.state.status()
    }
}

impl StreamingDecode for RowDecoder<'_> {
    type Error = DecodeError;
    fn next_batch(&mut self) -> Result<Option<(u32, zenpixels::PixelSlice<'_>)>, DecodeError> {
        let y = self.state.rows_decoded();
        let job = &self.job;
        if self
            .state
            .next(self.pixels.as_slice_mut().row_mut(0), &|| job.check())?
        {
            // One completed row on the job's pulse per row yielded.
            if let Some(pulse) = &job.pulse {
                pulse.as_pulse().advance(1);
            }
            Ok(Some((y, self.pixels.as_slice())))
        } else {
            Ok(None)
        }
    }
    fn info(&self) -> &zencodec::ImageInfo {
        &self.info
    }
}
