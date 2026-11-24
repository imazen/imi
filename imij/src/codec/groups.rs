//! Caller-drained encoding with the same common job description and limits.

use super::{EncodeRequest, encode_description, encode_description_for};
use crate::stream_encode::{GroupEncodeSession, GroupRect};
use crate::{Error, ImageInfo, Metadata};
use how_far::Report;
use zenpixels::{PixelDescriptor, PixelSlice};

/// Caller-driven lossless groups with bounded input and output storage.
///
/// Created by [`EncodeRequest::group_encoder`]. Drain [`Self::output`] using
/// [`Self::consume_output`], then supply exactly [`Self::next_group`]. No source
/// raster, complete encoded file, index or checksum is retained. Source/sink
/// buffers belong to the caller. Any observed operation error is sticky;
/// discard already-transferred output after failure.
pub struct GroupEncoder {
    job: EncodeRequest,
    session: GroupEncodeSession,
    descriptor: PixelDescriptor,
    info: ImageInfo,
    metadata: Metadata,
    input: u64,
    produced: u64,
    failed: Option<Error>,
}

impl EncodeRequest {
    /// Start caller-drained fixed-group lossless encoding.
    ///
    /// Select [`crate::ImiEncoderConfig::with_row_groups`], canvas dimensions and an input
    /// descriptor first. Color carriers belong in job metadata; each supplied
    /// group's descriptor/context must describe that same image. The common
    /// configuration, metadata policy, limits, stop token and progress apply.
    /// The configured working allowance includes one encoded group/header;
    /// unlike `Encoder::finish`, this executor never accumulates encoded output.
    /// Progress counts completed encoded rows. Cancellation is checked between
    /// groups and transfers, not inside the native group kernel.
    pub fn group_encoder(self) -> Result<GroupEncoder, Error> {
        self.validate()?;
        let cfg = self.group_config("group encoding mode")?;
        let (width, height) = self
            .canvas
            .ok_or(Error::InvalidInput("group canvas required"))?;
        let descriptor = self
            .input_descriptor
            .ok_or(Error::InvalidInput("group descriptor required"))?;
        if width == 0 || height == 0 {
            return Err(Error::InvalidInput("zero group canvas"));
        }
        self.limits
            .check_dimensions(width, height)
            .map_err(|_| Error::TooLarge)?;
        self.limits
            .check_total_pixels(u64::from(width) * u64::from(height))
            .map_err(|_| Error::TooLarge)?;
        self.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        let input = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(descriptor.bytes_per_pixel() as u64))
            .ok_or(Error::TooLarge)?;
        self.limits
            .check_input_size(input)
            .map_err(|_| Error::TooLarge)?;
        let planned =
            (cfg.max_working_bytes as u128) + core::mem::size_of::<GroupEncoder>() as u128;
        if self
            .limits
            .max_memory_bytes
            .is_some_and(|cap| planned > u128::from(cap))
        {
            return Err(Error::TooLarge);
        }
        admit_carriers(&self, None, cfg.max_working_bytes)?;
        let (info, metadata) = encode_description_for(
            descriptor,
            (width, height),
            None,
            &self.metadata,
            &self.policy,
            self.color_origin.as_ref(),
        )?;
        let session = GroupEncodeSession::with_groups(info.clone(), &metadata, cfg)?;
        let produced = session.output().len() as u64;
        self.limits
            .check_output_size(produced)
            .map_err(|_| Error::TooLarge)?;
        self.check_stop()?;
        Ok(GroupEncoder {
            job: self,
            session,
            descriptor,
            info,
            metadata,
            input: 0,
            produced,
            failed: None,
        })
    }
}

impl GroupEncoder {
    fn check(&mut self) -> Result<(), Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if let Err(error) = self.job.check_stop() {
            self.failed = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    fn retain(&mut self, result: Result<(), Error>) -> Result<(), Error> {
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    /// Pending bytes, bounded by the native header or one encoded group.
    pub fn output(&mut self) -> Result<&[u8], Error> {
        self.check()?;
        Ok(self.session.output())
    }

    /// Acknowledge any prefix already transferred to the caller's sink.
    pub fn consume_output(&mut self, count: usize) -> Result<(), Error> {
        self.check()?;
        let result = self.session.consume_output(count);
        self.retain(result)
    }

    /// Next rectangle, or None while output needs drainage/encoding is complete.
    pub fn next_group(&self) -> Option<GroupRect> {
        if self.failed.is_some() {
            None
        } else {
            self.session.next_group()
        }
    }

    /// Submit exactly the requested rectangle, with native row stride support.
    pub fn push_group(&mut self, pixels: PixelSlice<'_>) -> Result<(), Error> {
        self.check()?;
        let result = self.push(pixels);
        self.retain(result)
    }

    fn push(&mut self, pixels: PixelSlice<'_>) -> Result<(), Error> {
        let rect = self.session.next_group().ok_or(Error::InvalidInput(
            "group needs output drainage or is finished",
        ))?;
        if (pixels.width(), pixels.rows(), pixels.descriptor())
            != (rect.width, rect.height, self.descriptor)
        {
            return Err(Error::InvalidInput("group shape or descriptor mismatch"));
        }
        let input = self
            .input
            .checked_add(pixels.as_strided_bytes().len() as u64)
            .ok_or(Error::TooLarge)?;
        self.job
            .limits
            .check_input_size(input)
            .map_err(|_| Error::TooLarge)?;
        let working = self
            .job
            .group_config("group encoding mode")?
            .max_working_bytes;
        admit_carriers(
            &self.job,
            pixels.color_context().map(AsRef::as_ref),
            working,
        )?;
        let (mut info, metadata) = encode_description(
            &pixels,
            &self.job.metadata,
            &self.job.policy,
            self.job.color_origin.as_ref(),
        )?;
        info.width = self.info.width;
        info.height = self.info.height;
        if info != self.info || metadata != self.metadata {
            return Err(Error::InvalidInput("group description changed"));
        }
        self.session
            .push_group(pixels.as_strided_bytes(), pixels.stride())?;
        let produced = self
            .produced
            .checked_add(self.session.output().len() as u64)
            .ok_or(Error::TooLarge)?;
        self.job
            .limits
            .check_output_size(produced)
            .map_err(|_| Error::TooLarge)?;
        self.input = input;
        self.produced = produced;
        if rect.x + rect.width == self.info.width
            && let Some(progress) = &self.job.progress
        {
            progress.advance(u64::from(rect.height));
        }
        self.check()
    }

    /// Total admitted encoded bytes, including already-transferred prefixes.
    pub fn bytes_produced(&self) -> u64 {
        self.produced
    }

    /// Confirm that every group was supplied and all output was transferred.
    pub fn finish(mut self) -> Result<(), Error> {
        self.check()?;
        self.session.finish()
    }
}

// Bound copies before materializing native metadata, both at open and when a
// later group offers a context. Historical ICC is independent of active ICC.
fn admit_carriers(
    job: &EncodeRequest,
    context: Option<&zenpixels::ColorContext>,
    working: usize,
) -> Result<(), Error> {
    let active = job
        .metadata
        .icc_profile
        .as_ref()
        .map_or(0, |v| v.len())
        .max(context.and_then(|c| c.icc.as_ref()).map_or(0, |v| v.len()));
    let origin = if job.policy.embed_icc == Some(false) {
        0
    } else {
        job.color_origin
            .as_ref()
            .and_then(|o| o.icc.as_ref())
            .map_or(0, |v| v.len())
    };
    let exif = if job.policy.embed_exif == Some(false) {
        0
    } else {
        job.metadata.exif.as_ref().map_or(0, |v| v.len())
    };
    let xmp = if job.policy.embed_xmp == Some(false) {
        0
    } else {
        job.metadata.xmp.as_ref().map_or(0, |v| v.len())
    };
    let bytes = [active, origin, exif, xmp]
        .into_iter()
        .try_fold(0usize, |n, v| n.checked_add(v))
        .ok_or(Error::TooLarge)?;
    if bytes > working / 4 {
        return Err(Error::TooLarge);
    }
    Ok(())
}
