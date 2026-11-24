//! Retain the actual native preparation across shared job admission.
//!
//! Ordinary output uses the budgeted seekable buffer and appears at finish.
//! The opt-in lab one-pass profile publishes completed bands before acquiring
//! later input. Drained prefixes leave the retained buffer; final validation
//! remains separate from provisional output publication.

use super::bounded::{Budget, Memory, Store, codec_error};
use super::{EncodeRequest, FORMAT, ImageEncoder, encode_description_for, image_info_for};
use crate::Error;
use crate::stream_encode::{
    BoundedLossyPlan, BoundedLossySession, BoundedLossyStep, BoundedStream,
};
use alloc::{boxed::Box, sync::Arc};
use std::io::{Read, Seek, SeekFrom, Write};
use zencodec::encode::{
    ColorCarriers, EncodeDeclaration, EncodeOutput, EncodePlan, EncodeRows, OutputPhase,
    OutputTiming, PullProgress, RowDemand, SourcePasses,
};
use zenpixels::{ColorContext, PixelDescriptor, PixelSliceMut, sample::ExactDepth};

// This is the native coding pass in lossy/bounded.rs, not a generic scheduler
// interpretation of RowDemand::label. Every other native traversal precedes it.
const CODING_PASS: u8 = 2;

struct Declaration {
    width: u32,
    height: u32,
    descriptor: PixelDescriptor,
    color: Option<Arc<ColorContext>>,
    access: zencodec::output_patch::OutputAccess,
    exact_depth: Option<ExactDepth>,
}
impl Declaration {
    fn new(d: &EncodeDeclaration<'_>) -> Self {
        Self {
            width: d.width,
            height: d.height,
            descriptor: d.descriptor,
            color: d.color.map(|c| Arc::new(c.clone())),
            access: d.access,
            exact_depth: d.exact_depth,
        }
    }
    fn matches(&self, d: &EncodeDeclaration<'_>) -> bool {
        self.width == d.width
            && self.height == d.height
            && self.descriptor == d.descriptor
            && self.color.as_deref() == d.color
            && self.access == d.access
            && self.exact_depth == d.exact_depth
    }
}

pub(super) struct Prepared {
    declaration: Declaration,
    native: BoundedLossyPlan,
    facts: EncodePlan,
    budget: Budget,
    streaming: bool,
}

impl EncodeRequest {
    pub(super) fn plan_retained(&self, d: &EncodeDeclaration<'_>) -> Result<EncodePlan, Error> {
        self.validate()?;
        if !self.is_bounded() {
            return Err(Error::Unsupported(
                zencodec::UnsupportedOperation::EncodePlanning.name(),
            ));
        }
        // No caller cancellation callback runs under this private cache lock.
        let mut cache = self
            .prepared
            .lock()
            .map_err(|_| Error::InvalidInput("poisoned preparation"))?;
        if let Some(p) = cache.as_ref() {
            return if p.declaration.matches(d) {
                Ok(p.facts.clone())
            } else {
                Err(Error::InvalidInput(
                    "encode declaration changed after planning",
                ))
            };
        }
        d.validate_exact_depth()
            .map_err(|_| Error::Unsupported("exact-depth declaration"))?;
        if d.width == 0 || d.height == 0 {
            return Err(Error::InvalidInput("empty image"));
        }
        if self.canvas.is_some_and(|c| c != (d.width, d.height))
            || self.input_descriptor.is_some_and(|p| p != d.descriptor)
        {
            return Err(Error::InvalidInput("job and encode declaration disagree"));
        }
        self.limits
            .check_dimensions(d.width, d.height)
            .map_err(|_| Error::TooLarge)?;
        self.limits
            .check_total_pixels(u64::from(d.width) * u64::from(d.height))
            .map_err(|_| Error::TooLarge)?;
        self.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        let bytes = u64::from(d.width)
            .checked_mul(u64::from(d.height))
            .and_then(|v| v.checked_mul(d.descriptor.bytes_per_pixel() as u64))
            .ok_or(Error::TooLarge)?;
        self.limits
            .check_input_size(bytes)
            .map_err(|_| Error::TooLarge)?;

        // Keep the shared native prototype's admitted color domain until a
        // broader route has its own native qualification. Ordinary encode still
        // supports its existing formats. In particular, opaque ICC must not be
        // treated as proof that the bounded transform sees sRGB samples.
        let desc = d.descriptor;
        if !matches!(
            desc.channel_type(),
            zenpixels::ChannelType::U8 | zenpixels::ChannelType::U16
        ) || !matches!(
            desc.layout(),
            zenpixels::ChannelLayout::Gray
                | zenpixels::ChannelLayout::GrayAlpha
                | zenpixels::ChannelLayout::Rgb
                | zenpixels::ChannelLayout::Rgba
        ) || desc.transfer() != zenpixels::TransferFunction::Srgb
            || desc.primaries != zenpixels::ColorPrimaries::Bt709
            || desc.signal_range != zenpixels::SignalRange::Full
            || desc
                .alpha()
                .is_some_and(|a| a != zenpixels::AlphaMode::Straight)
            || d.color
                .is_some_and(|c| c.icc.is_some() || c.cicp != Some(zencodec::Cicp::SRGB))
            || self.metadata.icc_profile.is_some()
        {
            return Err(Error::Unsupported(
                "declared bounded input requires sRGB U8/U16 without ICC",
            ));
        }
        // Current color comes from the declaration, never from job metadata.
        let current = image_info_for(d.descriptor, (d.width, d.height), d.color, None)?;
        let signaled = image_info_for(
            d.descriptor,
            (d.width, d.height),
            d.color,
            self.metadata.cicp,
        )?;
        if current != signaled
            || self
                .metadata
                .diffuse_white
                .is_some_and(|white| d.color.and_then(|c| c.diffuse_white) != Some(white))
        {
            return Err(Error::InvalidInput(
                "job metadata would override current color",
            ));
        }
        // The old blocking estimate does not cover the retained session and
        // metadata owner. Refuse this route's unqualified cap, leaving ordinary
        // bounded encode behavior unchanged. No total-memory bound is inferred.
        if self.limits.max_memory_bytes.is_some() {
            return Err(Error::Unsupported("retained session working-memory limit"));
        }
        let budget = Budget::new(u64::MAX);
        let limit = u64::MAX;
        // Shared metadata uses Arc payloads; native Metadata owns Vec payloads.
        // This performs one explicit fallible conversion per accepted plan.
        // Repeated plans and startup never clone those native payloads.
        let (info, metadata) = encode_description_for(
            d.descriptor,
            (d.width, d.height),
            d.color,
            &self.metadata,
            &self.policy,
            self.color_origin.as_ref(),
        )?;
        #[cfg(feature = "lab")]
        let streaming = self.config.prefer_streaming() == Some(true);
        #[cfg(not(feature = "lab"))]
        let streaming = false;
        #[cfg(feature = "lab")]
        let native = if streaming {
            BoundedLossyPlan::one_pass(info, metadata, &self.config, limit, limit)
        } else {
            BoundedLossyPlan::v4(info, metadata, &self.config, limit)
        };
        #[cfg(not(feature = "lab"))]
        let native = BoundedLossyPlan::v4(info, metadata, &self.config, limit);
        let native = native.map_err(codec_error)?;
        let passes = native.passes();
        let info = native.info();
        // Inspect the actual owner after metadata/policy/origin lowering. Input
        // ICC absence alone cannot prove what a lowering step will emit. This
        // route has no qualified active-ICC transform, so refuse if one appears.
        if native.metadata().icc.is_some() {
            return Err(Error::Unsupported(
                "declared bounded output contains active ICC",
            ));
        }
        // Historical cORg remains ancillary; it is not an active ICC carrier.
        let carriers = ColorCarriers::new(
            None,
            Some(zencodec::Cicp::new(
                info.cicp.color_primaries,
                info.cicp.transfer_characteristics,
                info.cicp.matrix_coefficients,
                info.cicp.full_range,
            )),
        );
        let facts = EncodePlan::replay(
            SourcePasses::new(passes.min, Some(passes.max))
                .map_err(|_| Error::InvalidInput("native source pass range"))?,
            if streaming {
                OutputTiming::streaming(passes.band_rows)
            } else {
                OutputTiming::AT_FINISH
            },
            OutputPhase::FinalPass,
        )
        .with_carriers(carriers);
        facts
            .validate(d)
            .map_err(|_| Error::InvalidInput("encode plan declaration"))?;
        *cache = Some(Box::new(Prepared {
            declaration: Declaration::new(d),
            native,
            facts: facts.clone(),
            budget,
            streaming,
        }));
        Ok(facts)
    }
}

pub(super) struct Active {
    declaration: Declaration,
    prepared: Option<BoundedLossyPlan>,
    session: Option<BoundedLossySession>,
    index: Store,
    checksum: Option<Store>,
    output: Memory,
    output_len: u64,
    published: u64,
    streaming: bool,
    pending_source: bool,
    max_output: Option<u64>,
    label: Option<u8>,
    ordinal: u32,
    done: bool,
}

impl ImageEncoder {
    pub(super) fn begin_retained(
        &mut self,
        d: &EncodeDeclaration<'_>,
    ) -> Result<EncodePlan, Error> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        if self.rows.is_some() || self.spool.is_some() || self.retained.is_some() {
            return Err(Error::InvalidInput("encoding already started"));
        }
        self.job.check_stop()?;
        let cache = self
            .job
            .prepared
            .get_mut()
            .map_err(|_| Error::InvalidInput("poisoned preparation"))?;
        let p = cache.as_ref().ok_or(Error::Unsupported(
            zencodec::UnsupportedOperation::EncodePlanning.name(),
        ))?;
        if !p.declaration.matches(d) {
            return Err(Error::InvalidInput("startup declaration differs from plan"));
        }
        let scratch = self
            .job
            .scratch
            .as_ref()
            .ok_or(Error::InvalidInput("missing scratch"))?;
        // Prepare fallible I/O before consuming the admitted owner. On refusal
        // files are removed and the cached native preparation remains intact.
        let index = Store::open(scratch, p.budget.clone())?;
        let checksum = if self.job.config.checksum() {
            Some(Store::open(scratch, p.budget.clone())?)
        } else {
            None
        };
        let output = Memory::new(p.budget.clone());
        let p = *cache
            .take()
            .ok_or(Error::InvalidInput("missing preparation"))?;
        let facts = p.facts;
        self.retained = Some(Box::new(Active {
            declaration: p.declaration,
            prepared: Some(p.native),
            session: None,
            index,
            checksum,
            output,
            output_len: 0,
            published: 0,
            streaming: p.streaming,
            pending_source: false,
            max_output: self.job.limits.max_output_bytes,
            label: None,
            ordinal: 0,
            done: false,
        }));
        // Materialization is deferred to first pull: begin is transactionally
        // unchanged on error, while a native startup error is terminal execution.
        Ok(facts)
    }

    pub(super) fn pull_retained(
        &mut self,
        rows: &mut dyn EncodeRows,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<PullProgress, Error> {
        // This is additional per-call cancellation. The retained job is checked
        // independently below and remains attached to the native session.
        let pulse = pulse.unwrap_or(&how_far::NoPulse);
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let mut active = self.retained.take().ok_or(Error::Unsupported(
            zencodec::UnsupportedOperation::PullEncode.name(),
        ))?;
        // On unwind, active drops its native state and scratch; the executor
        // stays terminal. No caller code runs while holding the preparation lock.
        self.failed = Some(Error::InvalidInput("declared pull interrupted"));
        let result: Result<PullProgress, Error> = (|| {
            self.job.check_stop()?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            let result = active.pull(rows, pulse, &self.job)?;
            self.job.check_stop()?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            Ok(result)
        })();
        match result {
            Ok(progress) => {
                self.failed = None;
                self.retained = Some(active);
                Ok(progress)
            }
            Err(e) => {
                self.failed = Some(e.clone());
                Err(e)
            }
        }
    }
}

impl Active {
    fn pull(
        &mut self,
        rows: &mut dyn EncodeRows,
        pulse: &dyn zencodec::Pulse,
        job: &EncodeRequest,
    ) -> Result<PullProgress, Error> {
        if self.done {
            return Ok(PullProgress::Done);
        }
        if let Some(prepared) = self.prepared.take() {
            self.session = Some(prepared.start().map_err(codec_error)?);
        }
        let session = self
            .session
            .as_mut()
            .ok_or(Error::InvalidInput("missing native session"))?;
        let mut filled = false;
        loop {
            job.check_stop()?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            let step = if self.pending_source {
                self.pending_source = false;
                session
                    .pending_rows()
                    .ok_or(Error::InvalidInput("missing pending source"))?
            } else {
                session.step().map_err(codec_error)?
            };
            match step {
                BoundedLossyStep::Rows {
                    pass,
                    y0,
                    rows: bytes,
                } => {
                    if filled {
                        // The preceding fill has been completely coded. Return
                        // to the caller for publication before touching this
                        // next source request; step() would falsely answer it.
                        self.pending_source = true;
                        return Ok(PullProgress::Filled);
                    }
                    if self.label != Some(pass) {
                        if self.label.is_some() {
                            self.ordinal += 1;
                        }
                        self.label = Some(pass);
                    }
                    let d = &self.declaration;
                    let stride = (d.width as usize)
                        .checked_mul(d.descriptor.bytes_per_pixel())
                        .ok_or(Error::TooLarge)?;
                    if stride == 0 || bytes.len() % stride != 0 {
                        return Err(Error::InvalidInput("native band layout"));
                    }
                    let count = u32::try_from(bytes.len() / stride).map_err(|_| Error::TooLarge)?;
                    let mut view = PixelSliceMut::new(bytes, d.width, count, stride, d.descriptor)
                        .map_err(|_| Error::InvalidInput("native band layout"))?;
                    if let Some(c) = &d.color {
                        view = view.with_color_context(Arc::clone(c));
                    }
                    rows.fill(
                        RowDemand::new(self.ordinal, y0, view)
                            .with_label(pass)
                            .with_final_pass(pass == CODING_PASS),
                    )
                    .map_err(|_| Error::Read)?;
                    job.check_stop()?;
                    pulse.check().map_err(|_| Error::Cancelled)?;
                    // Raw trait callers must not be able to bypass this assertion
                    // by providing a filler which ignores ExactDepth. The checked
                    // driver still retains its own more detailed typed cause.
                    if let Some(exact) = d.exact_depth {
                        let view =
                            zenpixels::PixelSlice::new(bytes, d.width, count, stride, d.descriptor)
                                .map_err(|_| Error::InvalidInput("native band layout"))?;
                        if !exact
                            .holds_for(&view)
                            .map_err(|_| Error::Unsupported("exact-depth verification"))?
                        {
                            return Err(Error::InvalidInput(
                                "source contradicts exact-depth declaration",
                            ));
                        }
                    }
                    filled = true;
                }
                BoundedLossyStep::Write {
                    stream: BoundedStream::Output,
                    offset,
                    bytes,
                } => {
                    let end = offset
                        .checked_add(bytes.len() as u64)
                        .ok_or(Error::TooLarge)?;
                    if offset > self.output_len || (self.streaming && offset != self.output_len) {
                        return Err(Error::InvalidInput("native output order"));
                    }
                    if self.max_output.is_some_and(|cap| end > cap) {
                        return Err(Error::TooLarge);
                    }
                    self.output
                        .seek(SeekFrom::Start(
                            offset
                                .checked_sub(self.published)
                                .ok_or(Error::InvalidInput("write before consumed output"))?,
                        ))
                        .map_err(io_error)?;
                    self.output.write_all(bytes).map_err(io_error)?;
                    self.output_len = self.output_len.max(end);
                }
                BoundedLossyStep::Write {
                    stream,
                    offset,
                    bytes,
                } => {
                    let store = match stream {
                        BoundedStream::IndexScratch => &mut self.index,
                        BoundedStream::ChecksumScratch => self
                            .checksum
                            .as_mut()
                            .ok_or(Error::InvalidInput("missing checksum scratch"))?,
                        _ => return Err(Error::InvalidInput("native scratch stream")),
                    };
                    store.seek(SeekFrom::Start(offset)).map_err(io_error)?;
                    store.write_all(bytes).map_err(io_error)?;
                }
                BoundedLossyStep::Read {
                    stream,
                    offset,
                    buf,
                } => {
                    let store = match stream {
                        BoundedStream::IndexScratch => &mut self.index,
                        BoundedStream::ChecksumScratch => self
                            .checksum
                            .as_mut()
                            .ok_or(Error::InvalidInput("missing checksum scratch"))?,
                        _ => return Err(Error::InvalidInput("native scratch stream")),
                    };
                    store.seek(SeekFrom::Start(offset)).map_err(io_error)?;
                    store.read_exact(buf).map_err(io_error)?;
                }
                BoundedLossyStep::Done { len } => {
                    if len != self.output_len {
                        return Err(Error::InvalidInput("native output length"));
                    }
                    self.session = None;
                    self.done = true;
                    return Ok(if filled {
                        PullProgress::Filled
                    } else {
                        PullProgress::Done
                    });
                }
            }
        }
    }

    pub(super) fn pending_output(&self) -> &[u8] {
        if self.streaming {
            self.output.as_bytes()
        } else {
            &[]
        }
    }

    pub(super) fn consume_output(&mut self, count: usize) {
        if self.streaming {
            self.published += self.output.consume_prefix(count) as u64;
        }
    }

    pub(super) fn take_output(&mut self, into: &mut alloc::vec::Vec<u8>) {
        into.extend_from_slice(self.pending_output());
        self.consume_output(self.pending_output().len());
    }

    pub(super) fn finish(self) -> Result<EncodeOutput, Error> {
        if !self.done {
            return Err(Error::InvalidInput("incomplete declared passes"));
        }
        Ok(EncodeOutput::new(
            self.output.into_vec(),
            zencodec::ImageFormat::Custom(&FORMAT),
        ))
    }
}

fn io_error(e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::OutOfMemory {
        Error::TooLarge
    } else {
        Error::Read
    }
}

#[cfg(test)]
mod tests;
