//! Caller-fed primary rows, with explicit final metadata and validation.
use super::*;
use crate::decode_resources::feed::Budget;
use crate::parse::limits::Admission;
use crate::read::input::{Input, Rows};
use alloc::boxed::Box;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Mutex;

/// Progress of a caller-fed primary-image decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FeedStatus {
    /// Supply more compressed bytes, or call `finish_input` for explicit EOF.
    NeedInput,
    /// One provisional native row is available through `FeedDecoder::row`.
    Row { y: u32 },
    /// Explicit EOF, trailing records and all supported requested checks passed.
    /// Opaque future suites remain unsupported; this is not authentication.
    Complete,
}

struct Shared {
    input: Vec<u8>,
    start: usize,
    capacity: usize,
    eof: bool,
    row: Option<PixelBuffer>,
    ready: bool,
    y: u32,
    image: Option<ImageInfo>,
    failure: Option<Error>,
}
#[derive(Clone)]
struct Transport {
    shared: Arc<Mutex<Shared>>,
    budget: Budget,
}
impl Input for Transport {
    async fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        poll_fn(|_| {
            if let Err(error) = self.budget.check() {
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .failure = Some(error);
                return Poll::Ready(Err(std::io::Error::other("feed admission stopped")));
            }
            let mut s = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            let n = out.len().min(s.input.len() - s.start);
            if n != 0 {
                out[..n].copy_from_slice(&s.input[s.start..s.start + n]);
                s.start += n;
                Poll::Ready(Ok(n))
            } else if s.eof || out.is_empty() {
                Poll::Ready(Ok(0))
            } else {
                Poll::Pending
            }
        })
        .await
    }
    fn reserve<T>(&mut self, out: &mut Vec<T>, additional: usize) -> Result<(), Error> {
        let needed = out.len().checked_add(additional).ok_or(Error::TooLarge)?;
        if needed <= out.capacity() {
            return Ok(());
        }
        // Amortize small/open head reads, while charging the entire requested
        // capacity before reserve. Buffers are reused across record stripes.
        let target = needed
            .max(out.capacity().checked_mul(2).unwrap_or(needed))
            .max(8);
        let result: Result<(), Error> = (|| {
            let growth = (target - out.capacity())
                .checked_mul(size_of::<T>())
                .ok_or(Error::TooLarge)?;
            self.account(growth)?;
            out.try_reserve_exact(target - out.len())
                .map_err(|_| Error::TooLarge)?;
            if out.capacity() > target {
                self.account(
                    (out.capacity() - target)
                        .checked_mul(size_of::<T>())
                        .ok_or(Error::TooLarge)?,
                )?;
            }
            Ok(())
        })();
        if let Err(ref error) = result {
            self.shared
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .failure = Some(error.clone());
        }
        result
    }
    fn account(&mut self, bytes: usize) -> Result<(), Error> {
        let result = self.budget.capacity(bytes);
        if let Err(ref error) = result {
            self.shared
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .failure = Some(error.clone());
        }
        result
    }
    fn image(&mut self, info: &ImageInfo) -> Result<(), Error> {
        common_info(info, &Metadata::default())?;
        self.shared.lock().map_err(|_| Error::Cancelled)?.image = Some(info.clone());
        Ok(())
    }
    fn header_bytes(&self, bytes: u64) -> Result<(), Error> {
        self.budget.header_bytes(bytes)
    }
}
impl Rows for Transport {
    async fn row(&mut self, info: &ImageInfo, y: u32, pixels: &[u8]) -> Result<(), Error> {
        self.budget.check()?;
        let mut buffer = {
            let mut s = self.shared.lock().map_err(|_| Error::Cancelled)?;
            s.row.take()
        };
        if buffer.is_none() {
            // Color metadata may follow DATA. Describe storage, without assigning
            // CICP or ICC authority before final metadata is available.
            let (_, mut descriptor) = common_info(info, &Metadata::default())?;
            descriptor.transfer = zenpixels::TransferFunction::Unknown;
            descriptor.primaries = zenpixels::ColorPrimaries::Unknown;
            let bytes = descriptor
                .aligned_stride(info.width)
                .checked_add(descriptor.min_alignment() - 1)
                .ok_or(Error::TooLarge)?;
            self.budget.capacity(bytes)?;
            buffer =
                Some(PixelBuffer::try_new(info.width, 1, descriptor).map_err(|_| Error::TooLarge)?);
        }
        self.budget.copy_work(pixels.len())?;
        let mut buffer = buffer.unwrap();
        buffer.as_slice_mut().row_mut(0).copy_from_slice(pixels);
        {
            let mut s = self.shared.lock().map_err(|_| Error::Cancelled)?;
            s.image = Some(info.clone());
            s.y = y;
            s.row = Some(buffer);
            s.ready = true;
        }
        poll_fn(|_| {
            let s = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            if s.ready {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        self.budget.check()
    }
}

type Operation = Pin<Box<dyn Future<Output = Result<(ImageInfo, Metadata), Error>> + Send>>;

/// Bounded caller-fed decoding of primary-image native rows.
///
/// `push` copies only the accepted prefix into a bounded queue. `advance`
/// consumes it until input is needed or one row is available. It acknowledges
/// the previous row before progressing. `finish_input` marks explicit EOF;
/// temporary exhaustion never means EOF. No complete compressed file or decoded
/// raster is retained. The queue, one output row and codec state are owned.
///
/// Rows are provisional until `Complete`: late metadata, checksums and records
/// can still reject the file. Their pixel descriptors deliberately leave color
/// transfer/primaries unknown. Use final `info`/`native_metadata` to interpret
/// them; this decoder performs no color conversion. Failures and cancellation
/// are sticky. This extension does not implement the complete-input
/// `StreamingDecode` contract or decode animation frames after the primary.
/// Lab tracing is isolated from caller threads; this executor emits no trace.
pub struct FeedDecoder {
    job: DecodeRequest,
    transport: Transport,
    operation: Option<Operation>,
    row: Option<PixelBuffer>,
    status: FeedStatus,
    info: Option<zencodec::ImageInfo>,
    native: Option<DecodedMetadata>,
    failure: Option<Error>,
}
impl DecodeRequest {
    /// Accept compressed input incrementally, retaining at most `input_capacity`
    /// queued bytes. Common job limits/cancellation and native admission apply.
    /// Accepted bytes count immediately; codec/transport capacity is charged
    /// before allocation. A zero queue capacity is invalid.
    pub fn feed_decoder(self, input_capacity: usize) -> Result<FeedDecoder, DecodeError> {
        self.check()?;
        if input_capacity == 0 {
            return Err(Error::InvalidInput("zero feed capacity").into());
        }
        let mut policy = self.config.admission;
        if let Some(max) = self.limits.max_memory_bytes {
            policy = policy.cap_working(max);
        }
        let limits = policy.output_limits();
        let budget = Budget::new(policy, self.stop.clone(), self.limits)?;
        budget.capacity(size_of::<FeedDecoder>() + size_of::<Shared>() + 128)?;
        budget.capacity(input_capacity)?;
        let mut input = Vec::new();
        input
            .try_reserve_exact(input_capacity)
            .map_err(|_| Error::TooLarge)?;
        if input.capacity() > input_capacity {
            budget.capacity(input.capacity() - input_capacity)?;
        }
        let transport = Transport {
            budget: budget.clone(),
            shared: Arc::new(Mutex::new(Shared {
                input,
                start: 0,
                capacity: input_capacity,
                eof: false,
                row: None,
                ready: false,
                y: 0,
                image: None,
                failure: None,
            })),
        };
        let future =
            crate::read::read_owned(transport.clone(), limits, budget.clone(), transport.clone());
        budget.capacity(size_of_val(&future))?;
        let operation: Operation = Box::pin(future);
        Ok(FeedDecoder {
            job: self,
            transport,
            operation: Some(operation),
            row: None,
            status: FeedStatus::NeedInput,
            info: None,
            native: None,
            failure: None,
        })
    }
}
impl FeedDecoder {
    fn check(&mut self) -> Result<(), DecodeError> {
        if let Some(ref e) = self.failure {
            return Err(e.clone().into());
        }
        if let Err(e) = self.job.check() {
            return self.fail(e);
        }
        Ok(())
    }
    fn fail<T>(&mut self, error: Error) -> Result<T, DecodeError> {
        self.failure = Some(error.clone());
        self.operation = None;
        self.row = None;
        self.info = None;
        self.native = None;
        let mut s = self
            .transport
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        s.input.clear();
        s.row = None;
        s.ready = false;
        Err(error.into())
    }
    /// Copy an accepted prefix; return its byte count. Zero means queue full.
    /// The unaccepted suffix remains caller-owned. Call `advance` to drain.
    pub fn push(&mut self, bytes: &[u8]) -> Result<usize, DecodeError> {
        self.check()?;
        let result = (|| {
            let mut s = self.transport.shared.lock().map_err(|_| Error::Cancelled)?;
            if s.eof {
                return Err(Error::InvalidInput("input already finished"));
            }
            let n = bytes.len().min(s.capacity - (s.input.len() - s.start));
            self.transport.budget.input(n)?;
            if s.start != 0 {
                let at = s.start;
                s.input.drain(..at);
                s.start = 0;
            }
            s.input.extend_from_slice(&bytes[..n]);
            Ok(n)
        })();
        match result {
            Ok(n) => Ok(n),
            Err(e) => self.fail(e),
        }
    }
    /// Mark EOF after all accepted input. Further nonempty or empty pushes fail.
    pub fn finish_input(&mut self) -> Result<(), DecodeError> {
        self.check()?;
        self.transport
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .eof = true;
        Ok(())
    }
    /// Acknowledge the previous row and progress to one row, input wait or completion.
    pub fn advance(&mut self) -> Result<FeedStatus, DecodeError> {
        self.check()?;
        if self.status == FeedStatus::Complete {
            return Ok(self.status);
        }
        if let Some(row) = self.row.take() {
            let mut s = self
                .transport
                .shared
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            s.row = Some(row);
            s.ready = false;
        }
        let result = crate::trace::without(|| {
            self.operation
                .as_mut()
                .unwrap()
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
        });
        if let Some(error) = self.transport.budget.failure() {
            return self.fail(error);
        }
        let allocation_failure = self
            .transport
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .failure
            .clone();
        if let Some(error) = allocation_failure {
            return self.fail(error);
        }
        match result {
            Poll::Pending => {
                let mut s = self
                    .transport
                    .shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if s.ready {
                    self.row = s.row.take();
                    self.status = FeedStatus::Row { y: s.y };
                } else {
                    self.status = FeedStatus::NeedInput;
                }
            }
            Poll::Ready(result) => {
                self.operation = None;
                let failure = self
                    .transport
                    .shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .failure
                    .clone();
                let (image, mut metadata) = match result {
                    Ok(value) => value,
                    Err(e) => return self.fail(failure.unwrap_or(e)),
                };
                if metadata.icc.is_some() && !self.job.policy.resolve_icc(true) {
                    return self.fail(Error::Unsupported(
                        "dropping authoritative ICC without color conversion",
                    ));
                }
                if !self.job.policy.resolve_icc(true) {
                    metadata.set_color_origin(None)?;
                }
                if !self.job.policy.resolve_exif(true) {
                    metadata.exif = None;
                }
                if !self.job.policy.resolve_xmp(true) {
                    metadata.xmp = None;
                }
                let extra = self.transport.budget.usage().metadata_bytes;
                if let Err(e) = usize::try_from(extra)
                    .map_err(|_| Error::TooLarge)
                    .and_then(|n| self.transport.budget.capacity(n))
                {
                    return self.fail(e);
                }
                let info = match common_info(&image, &metadata) {
                    Ok((i, _)) => i,
                    Err(e) => return self.fail(e),
                };
                self.info = Some(info);
                self.native = Some(DecodedMetadata { image, metadata });
                self.status = FeedStatus::Complete;
            }
        }
        self.check()?;
        Ok(self.status)
    }
    /// Borrow the current provisional row until the next mutable operation.
    pub fn row(&mut self) -> Result<Option<zenpixels::PixelSlice<'_>>, DecodeError> {
        self.check()?;
        Ok(self.row.as_ref().map(PixelBuffer::as_slice))
    }
    /// Parsed native storage and dimensions. Color metadata is not final yet.
    pub fn native_header(&self) -> Option<ImageInfo> {
        self.transport
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .image
            .clone()
    }
    /// Final common image/color description, available only after `Complete`.
    pub fn info(&self) -> Option<&zencodec::ImageInfo> {
        self.info.as_ref()
    }
    /// Final native precision and policy-filtered metadata after `Complete`.
    pub fn native_metadata(&self) -> Option<&DecodedMetadata> {
        self.native.as_ref()
    }
    /// Cumulative admission, including accepted input and transport capacity.
    pub fn usage(&self) -> crate::decode_resources::DecodeUsage {
        self.transport.budget.usage()
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    fn fixture() -> (Vec<u8>, Vec<u8>, ImageInfo) {
        let info = ImageInfo::new(19, 7, crate::Channels::Rgb, SampleType::U8);
        let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 53) as u8)
            .collect();
        let file = crate::encode(
            &pixels,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &crate::EncodeConfig::default(),
        )
        .unwrap();
        (file, pixels, info)
    }
    #[test]
    fn caller_feed_waits_rows_and_explicit_end() {
        fn send_static<T: Send + 'static>(_: &T) {}
        let (file, pixels, info) = fixture();
        for capacity in [1, 7, 8192] {
            let mut decoder = DecodeConfig::default()
                .job()
                .feed_decoder(capacity)
                .unwrap();
            send_static(&decoder);
            let mut at = 0;
            let mut out = Vec::new();
            let mut row = 0;
            loop {
                match decoder.advance().unwrap() {
                    FeedStatus::NeedInput => {
                        assert_eq!(decoder.advance().unwrap(), FeedStatus::NeedInput);
                        if at == file.len() {
                            decoder.finish_input().unwrap();
                        } else {
                            at += decoder.push(&file[at..]).unwrap();
                        }
                    }
                    FeedStatus::Row { y } => {
                        assert_eq!(y, row);
                        row += 1;
                        let px = decoder.row().unwrap().unwrap();
                        assert_eq!(
                            px.descriptor().transfer,
                            zenpixels::TransferFunction::Unknown
                        );
                        out.extend_from_slice(px.row(0));
                        assert!(decoder.info().is_none());
                    }
                    FeedStatus::Complete => break,
                }
            }
            assert_eq!(out, pixels);
            assert_eq!(row, info.height);
            assert_eq!(decoder.native_metadata().unwrap().image, info);
            assert_eq!(decoder.usage().input_bytes, file.len() as u64);
            assert_eq!(decoder.advance().unwrap(), FeedStatus::Complete);
            assert!(decoder.row().unwrap().is_none());
        }
    }
    #[test]
    fn caller_feed_input_limit_and_failure_are_sticky() {
        let policy = crate::decode_resources::DecodePolicy::default().with_max_input_bytes(3);
        let mut d = DecodeConfig::default()
            .with_admission(policy)
            .job()
            .feed_decoder(7)
            .unwrap();
        assert_eq!(d.push(b"IMI").unwrap(), 3);
        assert!(matches!(
            d.push(b"J"),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        assert!(matches!(
            d.advance(),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        assert_eq!(d.usage().input_bytes, 3);
        assert!(matches!(
            d.finish_input(),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        assert!(matches!(d.row(), Err(DecodeError::Codec(Error::TooLarge))));
    }
    #[test]
    fn caller_feed_truncation_is_terminal() {
        let (file, _, _) = fixture();
        for cut in 0..file.len() {
            let mut d = DecodeConfig::default()
                .job()
                .feed_decoder(file.len())
                .unwrap();
            d.push(&file[..cut]).unwrap();
            d.finish_input().unwrap();
            let error = loop {
                match d.advance() {
                    Ok(FeedStatus::Row { .. }) => {}
                    Err(DecodeError::Codec(e)) => break e,
                    other => panic!("truncated input did not fail: {other:?}"),
                }
            };
            assert!(matches!(d.advance(), Err(DecodeError::Codec(e)) if e == error));
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod waiting_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Clone)]
    struct Flag(Arc<AtomicBool>);
    impl Stop for Flag {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(zencodec::enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    #[test]
    // Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
    #[allow(deprecated)]
    fn caller_feed_cancel_waiting_input_or_output_stays_cancelled() {
        for output in [false, true] {
            let flag = Flag(Arc::new(AtomicBool::new(false)));
            let mut d = DecodeConfig::default()
                .job()
                .with_stop(StopToken::new(flag.clone()))
                .feed_decoder(1024)
                .unwrap();
            if output {
                let info = ImageInfo::new(1, 1, crate::Channels::Gray, SampleType::U8);
                let file = crate::encode(
                    &[42],
                    1,
                    &info,
                    &Metadata::default(),
                    &crate::EncodeConfig::default(),
                )
                .unwrap();
                assert_eq!(d.push(&file).unwrap(), file.len());
                assert_eq!(d.advance().unwrap(), FeedStatus::Row { y: 0 });
            } else {
                assert_eq!(d.advance().unwrap(), FeedStatus::NeedInput);
            }
            flag.0.store(true, Ordering::SeqCst);
            assert!(matches!(d.row(), Err(DecodeError::Codec(Error::Cancelled))));
            flag.0.store(false, Ordering::SeqCst);
            assert!(matches!(
                d.advance(),
                Err(DecodeError::Codec(Error::Cancelled))
            ));
            assert!(matches!(
                d.push(&[]),
                Err(DecodeError::Codec(Error::Cancelled))
            ));
            assert!(d.operation.is_none());
        }
    }
    #[test]
    fn caller_feed_memory_and_header_admission_precede_input_growth() {
        assert!(matches!(
            DecodeConfig::default()
                .job()
                .with_limits(ResourceLimits::none().with_max_memory(1))
                .feed_decoder(1),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        let p = crate::decode_resources::DecodePolicy::default().with_max_header_bytes(1);
        let mut d = DecodeConfig::default()
            .with_admission(p)
            .job()
            .feed_decoder(32)
            .unwrap();
        let mut head = b"IMIJ\r\n\x87\n".to_vec();
        crate::put_varint(&mut head, 65535);
        d.push(&head).unwrap();
        assert!(matches!(
            d.advance(),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        assert_eq!(d.usage().input_bytes, head.len() as u64);
        assert!(d.operation.is_none());
    }
    #[test]
    fn caller_feed_can_move_threads_while_pending() {
        let d = DecodeConfig::default().job().feed_decoder(8).unwrap();
        let mut d = std::thread::spawn(move || {
            let mut d = d;
            assert_eq!(d.advance().unwrap(), FeedStatus::NeedInput);
            d.push(b"IMIJ").unwrap();
            assert_eq!(d.advance().unwrap(), FeedStatus::NeedInput);
            d
        })
        .join()
        .unwrap();
        d.finish_input().unwrap();
        assert!(matches!(
            d.advance(),
            Err(DecodeError::Codec(Error::NotImij))
        ));
    }
}

#[cfg(all(test, feature = "encode"))]
mod coverage_tests {
    use super::*;
    fn run(file: &[u8], config: DecodeConfig) -> Result<(Vec<u8>, DecodedMetadata), Error> {
        let mut d = config.job().feed_decoder(17).map_err(|e| match e {
            DecodeError::Codec(e) => e,
            _ => panic!("no sink"),
        })?;
        let mut at = 0;
        let mut output = Vec::new();
        let mut eof = false;
        loop {
            match d.advance() {
                Ok(FeedStatus::NeedInput) => {
                    assert!(!eof);
                    if at == file.len() {
                        d.finish_input().unwrap();
                        eof = true;
                    } else {
                        at += d.push(&file[at..]).unwrap();
                    }
                }
                Ok(FeedStatus::Row { .. }) => {
                    output.extend_from_slice(d.row().unwrap().unwrap().row(0))
                }
                Ok(FeedStatus::Complete) => {
                    return Ok((output, d.native_metadata().unwrap().clone()));
                }
                Err(DecodeError::Codec(e)) => return Err(e),
                Err(_) => panic!("no sink"),
            }
        }
    }
    #[test]
    fn caller_feed_baseline_storage_bits_and_late_icc() {
        use crate::Channels::*;
        for (channels, sample) in [
            (Gray, SampleType::U8),
            (GrayAlpha, SampleType::U16),
            (Rgb, SampleType::F16),
            (Rgba, SampleType::F32),
            (Oklab, SampleType::F32),
            (OklabAlpha, SampleType::F32),
            (Cmyk, SampleType::U8),
        ] {
            let info = ImageInfo::new(5, 3, channels, sample);
            let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                .map(|i| (i * 97 + 11) as u8)
                .collect();
            let mut file = crate::encode(
                &raw,
                info.row_bytes(),
                &info,
                &Metadata::default(),
                &crate::EncodeConfig::default(),
            )
            .unwrap();
            file.extend_from_slice(b"iCCP");
            crate::put_varint(&mut file, 5);
            file.extend_from_slice(b"test");
            let (pixels, metadata) = run(&file, DecodeConfig::default()).unwrap();
            assert_eq!(pixels, raw);
            assert_eq!(metadata.image, info);
            assert_eq!(metadata.metadata.icc, Some(b"test".to_vec()));
        }
    }
    #[test]
    fn caller_feed_many_stripes_reuse_transport_capacity() {
        let info = ImageInfo::new(31, 4096, crate::Channels::Rgb, SampleType::U8);
        let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 97 + i / 17) as u8)
            .collect();
        let file = crate::encode(
            &raw,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &crate::EncodeConfig {
                effort: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let cap = 2 << 20;
        let policy = crate::decode_resources::DecodePolicy::default().with_max_working_bytes(cap);
        let (pixels, _) = run(&file, DecodeConfig::default().with_admission(policy)).unwrap();
        assert_eq!(pixels, raw);
        let policy =
            crate::decode_resources::DecodePolicy::default().with_max_working_bytes(64 << 10);
        assert_eq!(
            run(&file, DecodeConfig::default().with_admission(policy)).err(),
            Some(Error::TooLarge)
        );
    }
    #[cfg(feature = "lab")]
    #[test]
    fn caller_feed_poll_preserves_caller_trace() {
        let mut d = DecodeConfig::default().job().feed_decoder(17).unwrap();
        crate::trace::start();
        crate::trace::note("before", "caller".into());
        assert_eq!(d.advance().unwrap(), FeedStatus::NeedInput);
        d.push(b"IMIJ").unwrap();
        assert_eq!(d.advance().unwrap(), FeedStatus::NeedInput);
        crate::trace::note("after", "caller".into());
        let events = crate::trace::finish();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], crate::trace::Ev::Note { key: "before", value } if value == "caller")
        );
        assert!(
            matches!(&events[1], crate::trace::Ev::Note { key: "after", value } if value == "caller")
        );
    }
}
