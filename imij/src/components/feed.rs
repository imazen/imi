//! Owned, caller-fed execution of the shared native component parser.
use alloc::{borrow::Cow, boxed::Box, sync::Arc, vec::Vec};
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::sync::Mutex;

use super::decode::{self, Output};
use super::{ComponentEvent, ComponentHeader};
use crate::Error;
use crate::codec::DecodeError;
use crate::decode_resources::{DecodePolicy, DecodeUsage, feed::Budget};
use crate::parse::limits::Admission;
use crate::read::input::Input;
use zencodec::decode::InputRequest;
use zenpixels::buffer::PixelStorage;
use zenpixels::plane::PlanarLayout;

/// Progress of the review-only native component feed decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ComponentFeedStatus {
    /// Accepted input is exhausted; supply more bytes or mark explicit EOF.
    NeedInput,
    /// One provisional declaration, row or group is available from `event`.
    Event,
    /// Explicit EOF and all requested final checks have passed. This is fused.
    Complete,
}

#[derive(Clone, Copy)]
struct Integrity {
    encoded: bool,
    decoded: bool,
}
enum Event {
    Declaration,
    Row {
        component: u8,
        y: u32,
        storage: PixelStorage,
        start: usize,
        width: usize,
    },
    Group {
        top: u32,
        end: u32,
    },
    Complete(Integrity),
}
struct Shared {
    input: Vec<u8>,
    start: usize,
    capacity: usize,
    wanted: usize,
    eof: bool,
    pending: Option<Event>,
    declaration: Option<ComponentHeader<'static>>,
    complete: Option<Integrity>,
    ready: bool,
    returned: Option<PixelStorage>,
}
#[derive(Clone)]
struct Transport {
    shared: Arc<Mutex<Shared>>,
    budget: Budget,
    retain_icc: bool,
}
impl Input for Transport {
    async fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        poll_fn(|_| {
            if self.budget.check().is_err() {
                return Poll::Ready(Err(std::io::Error::other("component feed stopped")));
            }
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            let n = out.len().min(shared.input.len() - shared.start);
            if n != 0 {
                out[..n].copy_from_slice(&shared.input[shared.start..shared.start + n]);
                shared.start += n;
                shared.wanted = 0;
                Poll::Ready(Ok(n))
            } else if shared.eof || out.is_empty() {
                shared.wanted = 0;
                Poll::Ready(Ok(0))
            } else {
                shared.wanted = out.len();
                Poll::Pending
            }
        })
        .await
    }
}

fn copy<T: Copy>(mut owned: Vec<T>, values: &[T], budget: &Budget) -> Result<Vec<T>, Error> {
    owned.clear();
    budget.copy_work(
        values
            .len()
            .checked_mul(size_of::<T>())
            .ok_or(Error::TooLarge)?,
    )?;
    decode::reserve(&mut owned, values.len(), budget)?;
    owned.extend_from_slice(values);
    Ok(owned)
}
fn own_header(
    header: &ComponentHeader<'_>,
    budget: &Budget,
    retain_icc: bool,
) -> Result<ComponentHeader<'static>, Error> {
    budget.canvas(header.width, header.height)?;
    if !retain_icc
        && header.icc.is_some()
        && header.color_authority == zenpixels::ColorAuthority::Icc
    {
        return Err(Error::Unsupported(
            "dropping authoritative ICC without color conversion",
        ));
    }
    let specs = copy(Vec::new(), header.layout.planes(), budget)?;
    let layout = PlanarLayout::new(specs, header.layout.subsampling(), header.layout.siting())
        .map_err(|_| Error::Corrupt("component feed layout"))?;
    let icc = if retain_icc {
        header
            .icc
            .as_deref()
            .map(|bytes| copy(Vec::new(), bytes, budget).map(Cow::Owned))
            .transpose()?
    } else {
        None
    };
    Ok(ComponentHeader {
        width: header.width,
        height: header.height,
        orientation: header.orientation,
        decode_speed: header.decode_speed,
        numeric_features: header.numeric_features,
        layout,
        cicp: header.cicp,
        alpha_mode: header.alpha_mode,
        color_authority: header.color_authority,
        icc,
        data_offset: header.data_offset,
        decoded_bytes: header.decoded_bytes,
        codings: header.codings,
        checksum: header.checksum,
    })
}
impl Transport {
    async fn publish(&mut self, event: Event) -> Result<(), Error> {
        self.budget.check()?;
        {
            let mut shared = self.shared.lock().map_err(|_| Error::Cancelled)?;
            if shared.ready || shared.pending.is_some() {
                return Err(Error::Corrupt("component feed event overlap"));
            }
            shared.pending = Some(event);
            shared.ready = true;
        }
        poll_fn(|_| {
            if self.shared.lock().unwrap_or_else(|p| p.into_inner()).ready {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        self.budget.check()
    }
}
impl Output for Transport {
    async fn event(&mut self, event: ComponentEvent<'_>) -> Result<(), Error> {
        self.budget.check()?;
        let event = match event {
            ComponentEvent::Declaration(header) => {
                let owned = own_header(header, &self.budget, self.retain_icc)?;
                self.shared
                    .lock()
                    .map_err(|_| Error::Cancelled)?
                    .declaration = Some(owned);
                Event::Declaration
            }
            ComponentEvent::Row { .. } => {
                return Err(Error::Corrupt("component feed row requires owned storage"));
            }
            ComponentEvent::Group { luma_top, luma_end } => Event::Group {
                top: luma_top,
                end: luma_end,
            },
            ComponentEvent::Complete {
                encoded_integrity_verified,
                decoded_attestation,
            } => {
                self.shared.lock().map_err(|_| Error::Cancelled)?.complete = Some(Integrity {
                    encoded: encoded_integrity_verified,
                    decoded: decoded_attestation,
                });
                return Ok(());
            }
        };
        self.publish(event).await
    }
    async fn row(
        &mut self,
        component: u8,
        y: u32,
        storage: &mut PixelStorage,
        start: usize,
        width: usize,
    ) -> Result<(), Error> {
        decode::storage_row(storage, start, width)?;
        // Move the actual reconstructed stripe, then get the same owner back
        // after the caller acknowledges this row. No sample copy or row pool.
        let storage_owned = core::mem::replace(storage, PixelStorage::U8(Vec::new()));
        self.publish(Event::Row {
            component,
            y,
            storage: storage_owned,
            start,
            width,
        })
        .await?;
        *storage = self
            .shared
            .lock()
            .map_err(|_| Error::Cancelled)?
            .returned
            .take()
            .ok_or(Error::Corrupt("component feed storage return missing"))?;
        Ok(())
    }
}

type Operation = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;

/// Review-only caller-fed native component decoding, created by
/// [`crate::codec::DecodeRequest::component_feed_decoder`].
///
/// The bounded compressed queue accepts prefixes; one typed event applies
/// backpressure until the next `advance`. It retains numeric stripe state,
/// the handed-off native stripe and declaration, never a complete output raster.
/// Declaration/row/group events are provisional. `Complete` requires explicit
/// EOF, the shared index and all requested integrity checks. No CMS, chroma
/// upsampling, packed conversion or multi-frame session is implied.
///
/// Job limits and owned cancellation are shared with the parser. Failures are
/// sticky and release suspended state; earlier caller-copied output remains
/// provisional. The native header preserves active ICC authority. Nonactive ICC
/// may be omitted by job policy; dropping authoritative ICC is refused. Other
/// ancillary metadata is not exposed by this typed declaration API.
pub struct ComponentFeedDecoder {
    transport: Transport,
    operation: Option<Operation>,
    status: ComponentFeedStatus,
    event: Option<Event>,
    header: Option<ComponentHeader<'static>>,
    failure: Option<Error>,
}
impl ComponentFeedDecoder {
    pub(crate) fn start(
        policy: DecodePolicy,
        budget: Budget,
        capacity: usize,
        retain_icc: bool,
    ) -> Result<Self, Error> {
        if capacity == 0 {
            return Err(Error::InvalidInput("zero component feed capacity"));
        }
        budget.capacity(size_of::<Self>() + size_of::<Shared>() + 2 * size_of::<usize>())?;
        budget.capacity(capacity)?;
        let mut input = Vec::new();
        input
            .try_reserve_exact(capacity)
            .map_err(|_| Error::TooLarge)?;
        if input.capacity() > capacity {
            budget.capacity(input.capacity() - capacity)?;
        }
        let transport = Transport {
            budget: budget.clone(),
            retain_icc,
            shared: Arc::new(Mutex::new(Shared {
                input,
                start: 0,
                capacity,
                wanted: 0,
                eof: false,
                pending: None,
                declaration: None,
                complete: None,
                ready: false,
                returned: None,
            })),
        };
        let mut output = transport.clone();
        let reader = transport.clone();
        let future =
            async move { decode::decode::<_, false>(reader, policy, &budget, &mut output).await };
        transport.budget.capacity(size_of_val(&future))?;
        Ok(Self {
            transport,
            operation: Some(Box::pin(future)),
            status: ComponentFeedStatus::NeedInput,
            event: None,
            header: None,
            failure: None,
        })
    }
    fn clear(&mut self) {
        self.operation = None;
        self.event = None;
        let mut shared = self
            .transport
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        shared.input = Vec::new();
        shared.start = 0;
        shared.wanted = 0;
        shared.pending = None;
        shared.declaration = None;
        shared.complete = None;
        shared.ready = false;
        shared.returned = None;
    }
    fn fail<T>(&mut self, error: Error) -> Result<T, DecodeError> {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        self.header = None;
        self.clear();
        Err(self.failure.as_ref().unwrap().clone().into())
    }
    fn check(&mut self) -> Result<(), DecodeError> {
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        if self.status != ComponentFeedStatus::Complete
            && let Err(error) = self.transport.budget.check()
        {
            return self.fail(error);
        }
        Ok(())
    }
    /// The bounded span of the parser's current pending read, after `NeedInput`.
    ///
    /// Uses the common byte-acquisition contract. Sequential adapters should read
    /// this span rather than fill a speculative buffer across later groups. The
    /// span never exceeds queue capacity. `push` may still accept larger offered
    /// prefixes. Before the first advance, with queued bytes, or at an event,
    /// completion or failure there is no pending request to acquire.
    pub fn input_request(&self) -> Option<InputRequest> {
        if self.status != ComponentFeedStatus::NeedInput || self.failure.is_some() {
            return None;
        }
        let length = {
            let shared = self
                .transport
                .shared
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if shared.eof || shared.start != shared.input.len() || shared.wanted == 0 {
                return None;
            }
            shared.wanted.min(shared.capacity)
        };
        Some(InputRequest::range(
            self.transport.budget.usage().input_bytes,
            length as u64,
        ))
    }

    /// Copy only the accepted prefix. Zero means the compressed queue is full.
    /// Unaccepted bytes stay caller-owned. Bytes are charged immediately once.
    pub fn push(&mut self, bytes: &[u8]) -> Result<usize, DecodeError> {
        self.check()?;
        if self.status == ComponentFeedStatus::Complete {
            return Err(Error::InvalidInput("component feed is complete").into());
        }
        let result = (|| {
            let mut shared = self.transport.shared.lock().map_err(|_| Error::Cancelled)?;
            if shared.eof {
                return Err(Error::InvalidInput("input already finished"));
            }
            let n = bytes
                .len()
                .min(shared.capacity - (shared.input.len() - shared.start));
            self.transport.budget.input(n)?;
            let start = shared.start;
            if start != 0 {
                shared.input.drain(..start);
                shared.start = 0;
            }
            shared.input.extend_from_slice(&bytes[..n]);
            Ok(n)
        })();
        match result {
            Ok(n) => Ok(n),
            Err(error) => self.fail(error),
        }
    }
    /// Mark actual EOF. Temporary queue exhaustion never completes the image.
    pub fn finish_input(&mut self) -> Result<(), DecodeError> {
        self.check()?;
        if self.status != ComponentFeedStatus::Complete {
            self.transport
                .shared
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .eof = true;
        }
        Ok(())
    }
    /// Acknowledge the preceding event and run to one event, input wait or completion.
    pub fn advance(&mut self) -> Result<ComponentFeedStatus, DecodeError> {
        self.check()?;
        if self.status == ComponentFeedStatus::Complete {
            return Ok(self.status);
        }
        let acknowledge = (|| {
            let mut shared = self.transport.shared.lock().map_err(|_| Error::Cancelled)?;
            if let Some(Event::Row { storage, .. }) = self.event.take()
                && shared.returned.replace(storage).is_some()
            {
                return Err(Error::Corrupt("component feed storage return overlap"));
            }
            shared.ready = false;
            Ok(())
        })();
        if let Err(error) = acknowledge {
            return self.fail(error);
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
        match result {
            Poll::Pending => {
                let mut shared = self
                    .transport
                    .shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                self.event = shared.pending.take();
                if matches!(self.event, Some(Event::Declaration)) {
                    self.header = shared.declaration.take();
                }
                self.status = if self.event.is_some() {
                    ComponentFeedStatus::Event
                } else {
                    ComponentFeedStatus::NeedInput
                };
            }
            Poll::Ready(Err(error)) => return self.fail(error),
            Poll::Ready(Ok(())) => {
                let complete = self
                    .transport
                    .shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .complete
                    .take();
                let Some(complete) = complete else {
                    return self.fail(Error::Corrupt("component feed completion missing"));
                };
                self.clear();
                self.event = Some(Event::Complete(complete));
                self.status = ComponentFeedStatus::Complete;
            }
        }
        Ok(self.status)
    }
    /// Borrow the current native typed event until the next mutable operation.
    pub fn event(&mut self) -> Result<Option<ComponentEvent<'_>>, DecodeError> {
        self.check()?;
        Ok(match self.event.as_ref() {
            None => None,
            Some(Event::Declaration) => {
                Some(ComponentEvent::Declaration(self.header.as_ref().ok_or(
                    Error::Corrupt("component feed declaration missing"),
                )?))
            }
            Some(Event::Row {
                component,
                y,
                storage,
                start,
                width,
            }) => Some(ComponentEvent::Row {
                component: *component,
                y: *y,
                samples: decode::storage_row(storage, *start, *width)?,
            }),
            Some(Event::Group { top, end }) => Some(ComponentEvent::Group {
                luma_top: *top,
                luma_end: *end,
            }),
            Some(Event::Complete(checks)) => Some(ComponentEvent::Complete {
                encoded_integrity_verified: checks.encoded,
                decoded_attestation: checks.decoded,
            }),
        })
    }
    /// Provisional native interpretation. No packed pixel descriptor is synthesized.
    pub fn header(&self) -> Option<&ComponentHeader<'_>> {
        self.header.as_ref()
    }
    /// Accepted input and conservative planned capacities/work, not process RSS.
    pub fn usage(&self) -> DecodeUsage {
        self.transport.budget.usage()
    }
}

#[cfg(test)]
mod tests;
