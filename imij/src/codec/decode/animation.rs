use super::*;
use crate::decode_resources::{DecodeSession, DecodeUsage};
use crate::parse::limits::Admission;
use zencodec::decode::{AnimationFrame, AnimationFrameDecoder, OwnedAnimationFrame};

/// Exact native item information retained alongside common millisecond timing.
/// Available from [`AnimationDecoder::frame_metadata`] and owned-frame extras.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DecodedFrameMetadata {
    pub image: ImageInfo,
    pub metadata: Metadata,
    pub duration: crate::FrameDuration,
    pub reference_index: Option<usize>,
}

/// Sequential, serial, full-canvas animation executor. Owns compressed input,
/// one canvas and only earlier rasters whose final reference consumer is still
/// ahead. It does not buffer all decoded frames or accept incremental input.
///
/// Job and per-call cancellation apply during parsing, reconstruction, hashes
/// and row copies. A failed operation is sticky because its canvas may be partial.
/// Native rational durations are preserved in frame metadata and shared frames.
/// Millisecond compatibility views truncate and saturate; zero-duration frames remain
/// visible. Duration admission sums per-frame milliseconds rounded upwards,
/// excluding loops; the delivered rational timeline is unchanged.
pub struct AnimationDecoder {
    job: DecodeRequest,
    decoder: crate::FrameDecoder<'static>,
    info: zencodec::ImageInfo,
    canvas: Option<PixelBuffer>,
    references: Vec<Option<PixelBuffer>>,
    retained_references: usize,
    last_uses: Vec<Option<usize>>,
    durations: Vec<crate::FrameDuration>,
    current: Option<DecodedFrameMetadata>,
    next: usize,
    failure: Option<Error>,
    usage: DecodeUsage,
    input_capacity: u64,
    fixed_capacity: u64,
    pixel_capacity: u64,
}

fn check(stop: Option<&StopToken>, call: Option<&dyn zencodec::Pulse>) -> Result<(), Error> {
    if stop.is_some_and(|s| s.check().is_err()) || call.is_some_and(|s| s.check().is_err()) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn pixel_capacity(info: &ImageInfo, descriptor: PixelDescriptor) -> Result<u64, Error> {
    descriptor
        .aligned_stride(info.width)
        .checked_mul(info.height as usize)
        .and_then(|n| n.checked_add(descriptor.min_alignment() - 1))
        .map(|n| n as u64)
        .ok_or(Error::TooLarge)
}

fn metadata(job: &DecodeRequest, source: &Metadata) -> Result<Metadata, Error> {
    if source.icc.is_some() && !job.policy.resolve_icc(true) {
        return Err(Error::Unsupported(
            "dropping authoritative ICC without color conversion",
        ));
    }
    let mut result = source.clone();
    if !job.policy.resolve_icc(true) {
        result.set_color_origin(None)?;
    }
    if !job.policy.resolve_exif(true) {
        result.exif = None;
    }
    if !job.policy.resolve_xmp(true) {
        result.xmp = None;
    }
    Ok(result)
}

fn copy_pixels(
    source: &PixelBuffer,
    target: &mut PixelBuffer,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let source = source.as_slice();
    let mut target = target.as_slice_mut();
    for y in 0..source.rows() {
        for (dst, src) in target
            .row_mut(y)
            .chunks_mut(64 * 1024)
            .zip(source.row(y).chunks(64 * 1024))
        {
            check()?;
            dst.copy_from_slice(src);
        }
    }
    check()
}

impl AnimationDecoder {
    pub(super) fn new(
        job: DecodeRequest,
        data: Cow<'_, [u8]>,
        _preferred: &[PixelDescriptor],
    ) -> Result<Self, DecodeError> {
        // Output preferences remain hints; preserve the exact native descriptor.
        job.header(&data)?;
        let input_capacity = match &data {
            Cow::Borrowed(bytes) => bytes.len(),
            Cow::Owned(bytes) => bytes.capacity(),
        } as u64;
        job.limits
            .check_memory(input_capacity)
            .map_err(|_| Error::TooLarge)?;
        let mut policy = job.config.admission;
        if let Some(max) = job.limits.max_memory_bytes {
            policy = policy.cap_working(max.checked_sub(input_capacity).ok_or(Error::TooLarge)?);
        }
        let stop = job.stop.clone();
        let session = DecodeSession::with_cancel(policy, || check(stop.as_ref(), None).is_err());
        session.admit_input(&data)?;
        let data = match data {
            Cow::Owned(bytes) => bytes,
            Cow::Borrowed(bytes) => {
                let mut copy = Vec::new();
                copy.try_reserve_exact(bytes.len())
                    .map_err(|_| Error::TooLarge)?;
                copy.extend_from_slice(bytes);
                copy
            }
        };
        let input_capacity = data.capacity() as u64;
        job.limits
            .check_memory(input_capacity)
            .map_err(|_| Error::TooLarge)?;
        if let Some(max) = job.limits.max_memory_bytes {
            policy = job
                .config
                .admission
                .cap_working(max.checked_sub(input_capacity).ok_or(Error::TooLarge)?);
        }
        let session = DecodeSession::resume(policy, session.usage(), || {
            check(stop.as_ref(), None).is_err()
        });
        let admission = session.admission();
        let decoder =
            crate::FrameDecoder::from_owned_admitted(data, &policy.output_limits(), &admission)?;
        if !matches!(
            decoder.kind(),
            crate::FrameKind::Still | crate::FrameKind::Animation
        ) {
            return Err(Error::Unsupported("pages use the native FrameDecoder API").into());
        }
        let count = u32::try_from(decoder.frame_count()).map_err(|_| Error::TooLarge)?;
        job.limits
            .check_frames(count)
            .map_err(|_| Error::TooLarge)?;
        let native = decoder.info();
        let total = u64::from(native.width)
            .checked_mul(u64::from(native.height))
            .and_then(|n| n.checked_mul(u64::from(count)))
            .ok_or(Error::TooLarge)?;
        job.limits
            .check_total_pixels(total)
            .map_err(|_| Error::TooLarge)?;
        let (last_uses, durations) = decoder.animation_plan(&admission)?;
        let ms = durations.iter().try_fold(0u128, |sum, &duration| {
            sum.checked_add(crate::codec::timing::admission_ms(duration)?)
                .ok_or(Error::TooLarge)
        })?;
        crate::codec::timing::check_limit(ms, &job.limits)?;
        let slots = decoder
            .frame_count()
            .checked_mul(core::mem::size_of::<Option<PixelBuffer>>())
            .ok_or(Error::TooLarge)?;
        admission.hint(slots)?;
        // Native metadata, per-frame extras and common Arc carriers can coexist.
        let metadata_capacity = session
            .usage()
            .metadata_bytes
            .checked_mul(4)
            .ok_or(Error::TooLarge)?;
        admission.hint(usize::try_from(metadata_capacity).map_err(|_| Error::TooLarge)?)?;
        let meta = metadata(&job, decoder.metadata())?;
        let (mut info, descriptor) = common_info(native, &meta)?;
        info.sequence = if decoder.kind() == crate::FrameKind::Still {
            zencodec::ImageSequence::Single
        } else {
            zencodec::ImageSequence::Animation {
                frame_count: Some(count),
                loop_count: Some(decoder.loop_count()),
                random_access: false,
            }
        };
        let pixel_capacity = pixel_capacity(native, descriptor)?;
        let fixed_capacity = session.usage().peak_planned_working_bytes;
        let reserved = input_capacity
            .checked_add(fixed_capacity)
            .and_then(|n| n.checked_add(pixel_capacity))
            .ok_or(Error::TooLarge)?;
        job.limits
            .check_memory(reserved)
            .map_err(|_| Error::TooLarge)?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(count as usize)
            .map_err(|_| Error::TooLarge)?;
        references.resize_with(count as usize, || None);
        let canvas = PixelBuffer::try_new(native.width, native.height, descriptor)
            .map_err(|_| Error::TooLarge)?
            .with_color_context(Arc::new(info.source_color.to_color_context()));
        job.check()?;
        let usage = session.usage();
        Ok(Self {
            job,
            decoder,
            info,
            canvas: Some(canvas),
            references,
            retained_references: 0,
            last_uses,
            durations,
            current: None,
            next: 0,
            failure: None,
            usage,
            input_capacity,
            fixed_capacity,
            pixel_capacity,
        })
    }

    pub fn frame_metadata(&self) -> Option<&DecodedFrameMetadata> {
        self.current.as_ref()
    }
    /// Cumulative admitted input, metadata, units and work, including failed calls.
    /// Working bytes exclude the separately budgeted retained input and rasters.
    pub fn usage(&self) -> DecodeUsage {
        self.usage
    }

    fn reserved(&self, additional: usize) -> Result<u64, Error> {
        let buffers = 1 + self.retained_references + additional;
        self.pixel_capacity
            .checked_mul(buffers as u64)
            .and_then(|n| n.checked_add(self.input_capacity))
            .and_then(|n| n.checked_add(self.fixed_capacity))
            .and_then(|n| {
                self.usage
                    .metadata_bytes
                    .checked_mul(4)
                    .and_then(|m| n.checked_add(m))
            })
            .ok_or(Error::TooLarge)
    }

    fn advance(
        &mut self,
        call: Option<&dyn zencodec::Pulse>,
        owned_copy: bool,
        copy_output: bool,
    ) -> Result<Option<usize>, Error> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.advance_inner(call, owned_copy, copy_output);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
            self.current = None;
        }
        result
    }

    fn advance_inner(
        &mut self,
        call: Option<&dyn zencodec::Pulse>,
        owned_copy: bool,
        copy_output: bool,
    ) -> Result<Option<usize>, Error> {
        let stop = self.job.stop.clone();
        let check = || check(stop.as_ref(), call);
        check()?;
        if self.next == self.durations.len() {
            return Ok(None);
        }
        let index = self.next;
        let keep = self.last_uses[index].is_some_and(|last| last > index + 1);
        let reserved = self.reserved(usize::from(keep) + usize::from(owned_copy))?;
        self.job
            .limits
            .check_memory(reserved)
            .map_err(|_| Error::TooLarge)?;
        let mut policy = self.job.config.admission;
        // Reference rasters are internal codec state, unlike the delivered
        // canvas. They consume the native working allowance even when no
        // common max-memory limit was supplied.
        let retained_working = self
            .fixed_capacity
            .checked_add(
                self.usage
                    .metadata_bytes
                    .checked_mul(4)
                    .ok_or(Error::TooLarge)?,
            )
            .and_then(|n| {
                self.pixel_capacity
                    .checked_mul((self.retained_references + usize::from(keep)) as u64)
                    .and_then(|p| n.checked_add(p))
            })
            .ok_or(Error::TooLarge)?;
        policy = policy.reserve_working(retained_working)?;
        if let Some(max) = self.job.limits.max_memory_bytes {
            policy = policy.cap_working(max.checked_sub(reserved).ok_or(Error::TooLarge)?);
        }
        let session = DecodeSession::resume(policy, self.usage, || check().is_err());
        let result = (|| {
            let admission = session.admission();
            let frame = self
                .decoder
                .next_frame_admitted(&admission)?
                .ok_or(Error::Corrupt("animation frame count"))?;
            let reference = self.decoder.reference_index();
            let native = self.decoder.info().clone();
            let bytes = native.checked_buffer_len().ok_or(Error::TooLarge)? as u128;
            // Admit copies before changing the canvas or invoking the sink.
            session.adapter_work(
                bytes
                    * (u128::from(keep)
                        + u128::from(copy_output)
                        + u128::from(reference.is_some_and(|r| r + 1 != index))),
            )?;
            let expanded = session
                .usage()
                .metadata_bytes
                .checked_sub(self.usage.metadata_bytes)
                .ok_or(Error::TooLarge)?;
            admission.hint(
                usize::try_from(expanded.checked_mul(4).ok_or(Error::TooLarge)?)
                    .map_err(|_| Error::TooLarge)?,
            )?;
            let meta = metadata(
                &self.job,
                self.decoder
                    .frame_metadata()
                    .ok_or(Error::Corrupt("missing frame metadata"))?,
            )?;
            let (info, descriptor) = common_info(&native, &meta)?;
            // Same sample layout; color authority may change with item metadata.
            let canvas = self
                .canvas
                .take()
                .ok_or(Error::Corrupt("missing animation canvas"))?
                .reinterpret(descriptor)
                .map_err(|_| Error::InvalidInput("animation pixel layout changes"))?
                .with_color_context(Arc::new(info.source_color.to_color_context()));
            self.canvas = Some(canvas);
            let canvas = self.canvas.as_mut().unwrap();
            if let Some(reference) = reference
                && reference + 1 != index
            {
                let previous = self
                    .references
                    .get(reference)
                    .and_then(Option::as_ref)
                    .ok_or(Error::Corrupt("missing earlier reference"))?;
                copy_pixels(previous, canvas, &check)?;
            }
            let stride = canvas.stride();
            self.decoder.decode_into_admitted(
                canvas.as_slice_mut().as_strided_bytes_mut(),
                stride,
                &admission,
                &check,
            )?;
            if let Some(reference) = reference
                && self.last_uses[reference] == Some(index)
                && self.references[reference].take().is_some()
            {
                self.retained_references -= 1;
            }
            if keep {
                let mut copy = PixelBuffer::try_new(native.width, native.height, descriptor)
                    .map_err(|_| Error::TooLarge)?;
                copy_pixels(canvas, &mut copy, &check)?;
                self.references[index] = Some(copy);
                self.retained_references += 1;
            }
            self.current = Some(DecodedFrameMetadata {
                image: native,
                metadata: meta,
                duration: frame.duration,
                reference_index: reference,
            });
            check()?;
            self.next += 1;
            // One completed frame on the job's pulse, as the animation
            // encoder reports pushed frames.
            if let Some(pulse) = &self.job.pulse {
                pulse.as_pulse().advance(1);
            }
            Ok(Some(index))
        })();
        self.usage = session.usage();
        result
    }
}

impl AnimationFrameDecoder for AnimationDecoder {
    type Error = DecodeError;
    fn wrap_sink_error(error: zencodec::decode::SinkError) -> DecodeError {
        DecodeError::Sink(error)
    }
    fn info(&self) -> &zencodec::ImageInfo {
        &self.info
    }
    fn frame_count(&self) -> Option<u32> {
        Some(self.durations.len() as u32)
    }
    fn loop_count(&self) -> Option<u32> {
        (self.decoder.kind() == crate::FrameKind::Animation).then(|| self.decoder.loop_count())
    }
    fn render_next_frame(
        &mut self,
        stop: Option<&dyn zencodec::Pulse>,
    ) -> Result<Option<AnimationFrame<'_>>, DecodeError> {
        let Some(index) = self.advance(stop, false, false)? else {
            return Ok(None);
        };
        Ok(Some(AnimationFrame::with_duration(
            self.canvas.as_ref().unwrap().as_slice(),
            crate::codec::timing::common(self.durations[index])?,
            index as u32,
        )))
    }
    fn render_next_frame_owned(
        &mut self,
        stop: Option<&dyn zencodec::Pulse>,
    ) -> Result<Option<OwnedAnimationFrame>, DecodeError> {
        let Some(index) = self.advance(stop, true, true)? else {
            return Ok(None);
        };
        let result: Result<_, Error> = (|| {
            let source = self.canvas.as_ref().unwrap();
            let pixels = source.as_slice();
            let mut copy = PixelBuffer::try_new(pixels.width(), pixels.rows(), pixels.descriptor())
                .map_err(|_| Error::TooLarge)?;
            if let Some(context) = source.color_context() {
                copy = copy.with_color_context(Arc::clone(context));
            }
            copy_pixels(source, &mut copy, &|| check(self.job.stop.as_ref(), stop))?;
            Ok(OwnedAnimationFrame::with_duration(
                copy,
                crate::codec::timing::common(self.durations[index])?,
                index as u32,
            )
            .with_extras(self.current.as_ref().unwrap().clone()))
        })();
        match result {
            Ok(frame) => Ok(Some(frame)),
            Err(error) => {
                self.failure = Some(error.clone());
                self.current = None;
                Err(error.into())
            }
        }
    }
    fn render_next_frame_to_sink(
        &mut self,
        stop: Option<&dyn zencodec::Pulse>,
        sink: &mut dyn DecodeRowSink,
    ) -> Result<Option<OutputInfo>, DecodeError> {
        let Some(_) = self.advance(stop, false, true)? else {
            return Ok(None);
        };
        let result = (|| {
            let pixels = self.canvas.as_ref().unwrap().as_slice();
            let (width, height, descriptor) = (pixels.width(), pixels.rows(), pixels.descriptor());
            check(self.job.stop.as_ref(), stop)?;
            sink.begin(width, height, descriptor)
                .map_err(DecodeError::Sink)?;
            for y in 0..height {
                check(self.job.stop.as_ref(), stop)?;
                let mut row = sink
                    .provide_next_buffer(y, 1, width, descriptor)
                    .map_err(DecodeError::Sink)?;
                if row.width() != width || row.rows() < 1 || row.descriptor() != descriptor {
                    return Err(Error::InvalidInput("row sink output description").into());
                }
                check(self.job.stop.as_ref(), stop)?;
                for (dst, src) in row
                    .row_mut(0)
                    .chunks_mut(64 * 1024)
                    .zip(pixels.row(y).chunks(64 * 1024))
                {
                    check(self.job.stop.as_ref(), stop)?;
                    dst.copy_from_slice(src);
                }
            }
            check(self.job.stop.as_ref(), stop)?;
            sink.finish().map_err(DecodeError::Sink)?;
            check(self.job.stop.as_ref(), stop)?;
            Ok(Some(OutputInfo::full_decode(width, height, descriptor)))
        })();
        if result.is_err() {
            self.failure = Some(Error::Read);
            self.current = None;
        }
        result
    }
}
