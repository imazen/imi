//! Executors for zencodec animation encoding.

use super::{EncodeRequest, FORMAT, encode_description};
use crate::{Error, FrameConfig, FrameDuration, FrameEncoder, ImageInfo, Metadata};
use zencodec::encode::{AnimationFrameEncoder, EncodeOutput};
use zencodec::enough::Stop;
use zenpixels::{PixelDescriptor, PixelSlice};

/// Full-canvas animation executor using the native frame encoder.
///
/// Input stride and native sample bits are preserved. Compressed frames and the
/// previous source frame (when inter coding is enabled) are buffered until finish. Working-memory caps
/// are refused. Lossless work checks cancellation during source copies, changed
/// rectangle scans and native analysis/coding; lossy search and reconstruction
/// currently check only at their boundaries. Progress counts completed native frames,
/// including unchanged frames. Any failed operation permanently fails this executor.
pub struct AnimationEncoder {
    job: EncodeRequest,
    encoder: Option<FrameEncoder>,
    description: Option<(PixelDescriptor, ImageInfo, Metadata)>,
    frames: u32,
    input_bytes: u64,
    total_pixels: u64,
    duration_ms: u128,
    failed: Option<Error>,
}

impl AnimationEncoder {
    pub(super) fn new(mut job: EncodeRequest) -> Result<Self, Error> {
        job.validate()?;
        if job.limits.max_memory_bytes.is_some() {
            return Err(Error::Unsupported("animation encoder working-memory limit"));
        }
        if job.config.row_groups().is_some() {
            return Err(Error::Unsupported("animation in group row encoder"));
        }
        if let Some((width, height)) = job.canvas {
            if width == 0 || height == 0 {
                return Err(Error::InvalidInput("zero animation canvas"));
            }
            job.limits
                .check_dimensions(width, height)
                .map_err(|_| Error::TooLarge)?;
        }
        job.configure_threads();
        Ok(Self {
            job,
            encoder: None,
            description: None,
            frames: 0,
            input_bytes: 0,
            total_pixels: 0,
            duration_ms: 0,
            failed: None,
        })
    }

    fn check_stop(&self, stop: Option<&dyn zencodec::Pulse>) -> Result<(), Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        self.job.check_stop()?;
        if stop.is_some_and(|s| s.check().is_err()) {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    fn push(
        &mut self,
        pixels: PixelSlice<'_>,
        frame_duration: FrameDuration,
        stop: Option<&dyn zencodec::Pulse>,
    ) -> Result<(), Error> {
        self.check_stop(stop)?;
        let limits = &self.job.limits;
        if self
            .job
            .canvas
            .is_some_and(|size| size != (pixels.width(), pixels.rows()))
        {
            return Err(Error::InvalidInput("animation canvas mismatch"));
        }
        limits
            .check_dimensions(pixels.width(), pixels.rows())
            .map_err(|_| Error::TooLarge)?;
        let frames = self.frames.checked_add(1).ok_or(Error::TooLarge)?;
        let input = self
            .input_bytes
            .checked_add(pixels.as_strided_bytes().len() as u64)
            .ok_or(Error::TooLarge)?;
        let total = self
            .total_pixels
            .checked_add(u64::from(pixels.width()) * u64::from(pixels.rows()))
            .ok_or(Error::TooLarge)?;
        let duration = self
            .duration_ms
            .checked_add(super::timing::admission_ms(frame_duration)?)
            .ok_or(Error::TooLarge)?;
        limits.check_frames(frames).map_err(|_| Error::TooLarge)?;
        limits
            .check_input_size(input)
            .map_err(|_| Error::TooLarge)?;
        limits
            .check_total_pixels(total)
            .map_err(|_| Error::TooLarge)?;
        super::timing::check_limit(duration, limits)?;
        let (info, metadata) = encode_description(
            &pixels,
            &self.job.metadata,
            &self.job.policy,
            self.job.color_origin.as_ref(),
        )?;
        let description = (pixels.descriptor(), info, metadata);
        if let Some(first) = &self.description {
            if first != &description {
                return Err(Error::InvalidInput("animation pixel description changed"));
            }
        } else {
            let mut config = FrameConfig::new(self.job.config.clone());
            config.loop_count = self.job.loop_count.unwrap_or(0);
            self.encoder = Some(FrameEncoder::animation(
                description.1.clone(),
                description.2.clone(),
                config,
            ));
            self.description = Some(description);
        }
        let progress = self
            .job
            .progress
            .as_ref()
            .map_or(&how_far::NoPulse as &dyn how_far::Pulse, |p| p.as_pulse());
        let job_stop = how_far::WithStop::borrowed(progress, &self.job.stop);
        let frame_stop = how_far::WithStop::borrowed(&job_stop, &stop);
        // Native stages retain cancellation without consuming the caller's
        // completed-frame count or splitting its leaf repeatedly.
        let native = how_far::WithStop::borrowed(&how_far::NoPulse, &frame_stop);
        self.encoder
            .as_mut()
            .ok_or(Error::InvalidInput("missing animation encoder"))?
            .push_frame_checked(
                pixels.as_strided_bytes(),
                pixels.stride(),
                frame_duration,
                &native,
            )?;
        self.check_stop(stop)?;
        self.frames = frames;
        self.input_bytes = input;
        self.total_pixels = total;
        self.duration_ms = duration;
        progress.advance(1);
        self.check_stop(stop)?;
        Ok(())
    }
}

impl AnimationFrameEncoder for AnimationEncoder {
    type Error = Error;
    fn reject(op: zencodec::UnsupportedOperation) -> Error {
        Error::Unsupported(op.name())
    }
    fn push_frame(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ms: u32,
        stop: Option<&dyn zencodec::Pulse>,
    ) -> Result<(), Error> {
        let result = self.push(pixels, FrameDuration::from_millis(duration_ms), stop);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
    fn push_frame_timed(
        &mut self,
        pixels: PixelSlice<'_>,
        duration: zencodec::animation::FrameDuration,
        stop: Option<&dyn zencodec::Pulse>,
    ) -> Result<zencodec::animation::FrameDuration, Error> {
        let result = self.push(
            pixels,
            FrameDuration {
                num: duration.numerator(),
                den: duration.denominator(),
            },
            stop,
        );
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        // IMIJ stores the rational duration as given: nothing is rounded.
        result.map(|()| duration)
    }
    fn finish(mut self, stop: Option<&dyn zencodec::Pulse>) -> Result<EncodeOutput, Error> {
        self.check_stop(stop)?;
        let progress = self
            .job
            .progress
            .as_ref()
            .map_or(&how_far::NoPulse as &dyn how_far::Pulse, |p| p.as_pulse());
        let job_stop = how_far::WithStop::borrowed(progress, &self.job.stop);
        let stop = how_far::WithStop::borrowed(&job_stop, &stop);
        let data = self
            .encoder
            .take()
            .ok_or(Error::InvalidInput("no frames"))?
            .finish_checked(&stop)?;
        stop.check().map_err(|_| Error::Cancelled)?;
        self.job
            .limits
            .check_output_size(data.len() as u64)
            .map_err(|_| Error::TooLarge)?;
        Ok(EncodeOutput::new(
            data,
            zencodec::ImageFormat::Custom(&FORMAT),
        ))
    }
}
