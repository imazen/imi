//! Sans-IO cooperative session wrapper (`Session`, `SessionPlan`, `Step`) over `StreamIo`.

use super::*;

/// One pending request of a [`Session`]: what the encode waits for.
#[cfg(feature = "std")]
pub(super) enum Request {
    Idle,
    Rows {
        pass: u8,
        y0: usize,
        buf: Vec<u8>,
    },
    Write {
        stream: Stream,
        at: u64,
        buf: Vec<u8>,
    },
    Read {
        stream: Stream,
        at: u64,
        buf: Vec<u8>,
    },
}

/// Shared between a [`Session`] and the [`Port`] its encode runs on.
#[cfg(feature = "std")]
pub(super) struct Mailbox {
    pub(super) request: Request,
    /// The caller has handled `request` (its buffer holds read results).
    pub(super) answered: bool,
}

// Guards never span an await, a native poll, caller work or buffer destruction.
// A poisoned mailbox cannot provide a trustworthy answer; fail the session.
#[cfg(feature = "std")]
pub(super) fn lock_mailbox(
    mail: &std::sync::Mutex<Mailbox>,
) -> Result<std::sync::MutexGuard<'_, Mailbox>, crate::stream_encode::StreamEncodeError> {
    mail.lock()
        .map_err(|_| Error::Corrupt("bounded session mailbox poisoned").into())
}

/// [`StreamIo`] that posts each access to the mailbox and waits until the
/// session's caller has handled it.
#[cfg(feature = "std")]
pub(super) struct Port {
    pub(super) mail: std::sync::Arc<std::sync::Mutex<Mailbox>>,
    /// Reused request buffer.
    pub(super) spare: Vec<u8>,
    #[cfg(feature = "lab")]
    pub(super) output_limit: Option<u64>,
}

#[cfg(feature = "std")]
impl Port {
    /// Posts `request`, waits for the answer, and returns its buffer.
    pub(super) async fn ask(
        &mut self,
        request: Request,
    ) -> Result<Vec<u8>, crate::stream_encode::StreamEncodeError> {
        {
            let previous = {
                let mut m = lock_mailbox(&self.mail)?;
                m.answered = false;
                core::mem::replace(&mut m.request, request)
            };
            drop(previous);
        }
        core::future::poll_fn(|_| {
            let mut m = match lock_mailbox(&self.mail) {
                Ok(m) => m,
                Err(error) => return core::task::Poll::Ready(Err(error)),
            };
            if !m.answered {
                return core::task::Poll::Pending;
            }
            match core::mem::replace(&mut m.request, Request::Idle) {
                Request::Rows { buf, .. }
                | Request::Write { buf, .. }
                | Request::Read { buf, .. } => core::task::Poll::Ready(Ok(buf)),
                Request::Idle => core::task::Poll::Ready(Err(Error::Corrupt(
                    "bounded session answered nothing",
                )
                .into())),
            }
        })
        .await
    }

    pub(super) fn buffer(&mut self, len: usize) -> Result<Vec<u8>, Error> {
        let mut buf = core::mem::take(&mut self.spare);
        buf.clear();
        buf.try_reserve(len).map_err(|_| Error::TooLarge)?;
        Ok(buf)
    }
}

#[cfg(feature = "std")]
impl StreamIo for Port {
    async fn source(
        &mut self,
        pass: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        // The band itself goes to the caller and comes back filled.
        let buf = core::mem::take(out);
        let len = buf.len();
        *out = self.ask(Request::Rows { pass, y0, buf }).await?;
        if out.len() != len {
            return Err(Error::InvalidInput("bounded session row buffer length").into());
        }
        Ok(())
    }

    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        #[cfg(feature = "lab")]
        if stream == Stream::Output
            && self.output_limit.is_some_and(|limit| {
                at.checked_add(bytes.len() as u64)
                    .is_none_or(|end| end > limit)
            })
        {
            return Err(Error::TooLarge.into());
        }
        let mut buf = self.buffer(bytes.len())?;
        buf.extend_from_slice(bytes);
        self.spare = self.ask(Request::Write { stream, at, buf }).await?;
        Ok(())
    }

    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        out: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let mut buf = self.buffer(out.len())?;
        buf.resize(out.len(), 0);
        let buf = self.ask(Request::Read { stream, at, buf }).await?;
        out.copy_from_slice(&buf);
        self.spare = buf;
        Ok(())
    }
}

/// What a [`Session`] needs next.
#[cfg(feature = "std")]
pub(crate) enum Step<'a> {
    /// Fill `rows` with source rows from `y0` (whole rows, tightly packed),
    /// for `pass`.
    Rows {
        pass: u8,
        y0: usize,
        rows: &'a mut [u8],
    },
    /// Write `bytes` at offset `at` of `stream`.
    Write {
        stream: Stream,
        at: u64,
        bytes: &'a [u8],
    },
    /// Read `buf.len()` bytes at offset `at` of `stream` into `buf`.
    Read {
        stream: Stream,
        at: u64,
        buf: &'a mut [u8],
    },
    /// The encode is complete: the output is `len` bytes long.
    Done { len: u64 },
}

/// The exact validated native preparation, with sample tables and session state
/// deferred until start. Metadata is moved in, never copied during preparation.
#[cfg(feature = "std")]
pub(crate) struct SessionPlan {
    pub(super) preparation: NativePreparation,
    pub(super) info: ImageInfo,
    pub(super) meta: Metadata,
    pub(super) cfg: LossyConfig,
    pub(super) index: StreamIndex,
    pub(super) max_index_bytes: u64,
    pub(super) passes: (u32, u32, u32),
    pub(super) mode: Preparation,
}

#[cfg(feature = "std")]
impl SessionPlan {
    pub(crate) fn new(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        Self::prepared(
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            Preparation::Measured,
        )
    }

    #[cfg(feature = "lab")]
    pub(crate) fn one_pass(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        one_pass::admit(&info, &meta, &cfg, max_index_bytes, max_working_bytes)?;
        Self::prepared(
            info,
            meta,
            cfg,
            StreamIndex::Trailing,
            max_index_bytes,
            Preparation::OnePass,
        )
    }

    #[cfg(feature = "lab")]
    pub(crate) fn budgeted(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
        limits: one_pass::rate::Limits,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        one_pass::rate::admit(
            &info,
            &meta,
            &cfg,
            max_index_bytes,
            max_working_bytes,
            limits,
        )?;
        Self::prepared(
            info,
            meta,
            cfg,
            StreamIndex::Trailing,
            max_index_bytes,
            Preparation::Budgeted(limits),
        )
    }

    pub(super) fn prepared(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
        mode: Preparation,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        let preparation = prepare_plan(&info, &cfg)?;
        let (preparation, min, max) = match mode {
            #[cfg(all(test, feature = "lab"))]
            Preparation::StudyFirstBandDc => {
                unreachable!("study uses the private stream driver")
            }
            Preparation::Measured => {
                let (min, max) = preparation.plan.source_passes(&cfg);
                (preparation, min, max)
            }
            #[cfg(feature = "lab")]
            Preparation::OnePass | Preparation::Budgeted(_) => {
                let NativePreparation {
                    plan,
                    lut16_transfer,
                    precision_input,
                } = preparation;
                (
                    NativePreparation {
                        plan: one_pass::configure(plan, &cfg)?,
                        lut16_transfer,
                        precision_input,
                    },
                    1,
                    1,
                )
            }
        };
        validate_stream_plan(
            &preparation.plan,
            &cfg,
            index,
            cfg.checksum.then_some(0),
            max_index_bytes,
        )?;
        let band_rows = (preparation.plan.work * 8).min(preparation.plan.g.h) as u32;
        Ok(Self {
            preparation,
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            passes: (min, max, band_rows),
            mode,
        })
    }

    pub(crate) fn passes(&self) -> (u32, u32, u32) {
        self.passes
    }
    pub(crate) fn info(&self) -> &ImageInfo {
        &self.info
    }
    pub(crate) fn metadata(&self) -> &Metadata {
        &self.meta
    }

    pub(crate) fn start(self) -> Result<Session, crate::stream_encode::StreamEncodeError> {
        Session::start_planned(
            self.info,
            self.meta,
            self.cfg,
            self.index,
            self.max_index_bytes,
            self.mode,
            self.preparation.materialize()?,
            self.passes,
        )
    }
}

/// A bounded lossy encode as a state machine. No lock spans native work or a
/// returned request. The suspended future and its owned request buffers are Send.
#[cfg(feature = "std")]
pub(crate) struct Session {
    pub(super) run: Option<
        core::pin::Pin<
            alloc::boxed::Box<
                dyn core::future::Future<
                        Output = Result<u64, crate::stream_encode::StreamEncodeError>,
                    > + Send,
            >,
        >,
    >,
    pub(super) mail: std::sync::Arc<std::sync::Mutex<Mailbox>>,
    pub(super) held: Option<Request>,
    pub(super) done: Option<u64>,
    pub(super) failed: bool,
    // The first explicit caller failure is returned unchanged on the next step.
    pub(super) failure: Option<crate::stream_encode::StreamEncodeError>,
    pub(super) passes: (u32, u32, u32),
}

#[cfg(feature = "std")]
impl Session {
    pub(crate) fn new(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        Self::prepared(
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            Preparation::Measured,
        )
    }

    #[cfg(feature = "lab")]
    pub(crate) fn one_pass(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        SessionPlan::one_pass(info, meta, cfg, max_index_bytes, max_working_bytes)?.start()
    }

    #[cfg(feature = "lab")]
    pub(crate) fn budgeted(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
        limits: one_pass::rate::Limits,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        SessionPlan::budgeted(info, meta, cfg, max_index_bytes, max_working_bytes, limits)?.start()
    }

    pub(super) fn prepared(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
        preparation: Preparation,
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        // Refuse before the first request what the encode would refuse.
        let p = plan(&info, &cfg)?;
        let (p, min, max) = match preparation {
            #[cfg(all(test, feature = "lab"))]
            Preparation::StudyFirstBandDc => {
                unreachable!("study uses the private stream driver")
            }
            Preparation::Measured => {
                let (min, max) = p.source_passes(&cfg);
                (p, min, max)
            }
            #[cfg(feature = "lab")]
            Preparation::OnePass | Preparation::Budgeted(_) => {
                // Configure first: one-pass source bands follow the stripe
                // height, not the measured encoder's smaller work bands.
                (one_pass::configure(p, &cfg)?, 1, 1)
            }
        };
        let passes = (min, max, (p.work * 8).min(p.g.h) as u32);
        Self::start_planned(
            info,
            meta,
            cfg,
            index,
            max_index_bytes,
            preparation,
            p,
            passes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_planned(
        info: ImageInfo,
        meta: Metadata,
        cfg: LossyConfig,
        index: StreamIndex,
        max_index_bytes: u64,
        preparation: Preparation,
        p: Plan,
        passes: (u32, u32, u32),
    ) -> Result<Self, crate::stream_encode::StreamEncodeError> {
        let mail = std::sync::Arc::new(std::sync::Mutex::new(Mailbox {
            request: Request::Idle,
            answered: false,
        }));
        let mut port = Port {
            mail: mail.clone(),
            spare: Vec::new(),
            #[cfg(feature = "lab")]
            output_limit: match preparation {
                Preparation::Budgeted(limits) => Some(limits.frame_bytes),
                _ => None,
            },
        };
        let checksum = cfg.checksum.then_some(0);
        let run = alloc::boxed::Box::pin(async move {
            encode_planned_stream_io(
                p,
                &info,
                &meta,
                &cfg,
                &mut port,
                index,
                0,
                checksum,
                max_index_bytes,
                preparation,
            )
            .await
        });
        Ok(Self {
            run: Some(run),
            mail,
            held: None,
            done: None,
            failed: false,
            failure: None,
            passes,
        })
    }

    pub(crate) fn passes(&self) -> (u32, u32, u32) {
        self.passes
    }

    pub(super) fn answer(&mut self) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if let Some(request) = self.held.take() {
            let previous = {
                let mut mail = lock_mailbox(&self.mail)?;
                mail.answered = true;
                core::mem::replace(&mut mail.request, request)
            };
            drop(previous);
        }
        Ok(())
    }

    // Native polling happens here, outside the mutex. All errors flow through
    // step(), which drops the future/buffers and prevents any subsequent poll.
    pub(super) fn advance(&mut self) -> Result<(), crate::stream_encode::StreamEncodeError> {
        use core::task::{Context, Poll, Waker};
        self.answer()?;
        match self
            .run
            .as_mut()
            .expect("active session")
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => {
                self.done = Some(result?);
                self.run = None;
                return Ok(());
            }
            Poll::Pending => {}
        }
        let request = {
            let mut mail = lock_mailbox(&self.mail)?;
            core::mem::replace(&mut mail.request, Request::Idle)
        };
        if matches!(request, Request::Idle) {
            return Err(Error::Corrupt("bounded session suspended without a request").into());
        }
        self.held = Some(request);
        Ok(())
    }

    pub(super) fn terminate(&mut self) {
        self.failed = true;
        self.run = None;
        self.held = None;
        // Once the future is dropped this is the only Arc. Recover poisoned
        // storage solely to release its payload, never to resume the protocol.
        let request = {
            let mut mail = self.mail.lock().unwrap_or_else(|e| e.into_inner());
            core::mem::replace(&mut mail.request, Request::Idle)
        };
        drop(request);
    }

    /// Borrow the current unanswered row request without completing it. The
    /// common adapter uses this after publishing the preceding completed band.
    pub(crate) fn pending_rows(&mut self) -> Option<(u8, usize, &mut [u8])> {
        if self.failed || self.done.is_some() {
            return None;
        }
        match self.held.as_mut()? {
            Request::Rows { pass, y0, buf } => Some((*pass, *y0, buf)),
            _ => None,
        }
    }

    /// Completes the previous step. Done is fused; after the first typed error,
    /// later calls return a state error without polling the completed future.
    pub(crate) fn step(&mut self) -> Result<Step<'_>, crate::stream_encode::StreamEncodeError> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        if self.failed {
            return Err(Error::Corrupt("bounded session failed").into());
        }
        if let Some(len) = self.done {
            return Ok(Step::Done { len });
        }
        if let Err(error) = self.advance() {
            self.terminate();
            return Err(error);
        }
        if let Some(len) = self.done {
            return Ok(Step::Done { len });
        }
        Ok(match self.held.as_mut() {
            Some(Request::Rows { pass, y0, buf }) => Step::Rows {
                pass: *pass,
                y0: *y0,
                rows: buf,
            },
            Some(Request::Write { stream, at, buf }) => Step::Write {
                stream: *stream,
                at: *at,
                bytes: buf,
            },
            Some(Request::Read { stream, at, buf }) => Step::Read {
                stream: *stream,
                at: *at,
                buf,
            },
            _ => unreachable!("advance validated the request"),
        })
    }

    /// Latch the first caller failure and release suspended native buffers now.
    /// Even a future that swallowed an I/O error cannot be resumed after this.
    pub(crate) fn fail(&mut self, error: crate::stream_encode::StreamEncodeError) {
        if self.failed || self.done.is_some() {
            return;
        }
        self.failure = Some(error);
        self.terminate();
    }
}
