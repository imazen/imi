//! Owned accounting for a suspended executor. Blocking sessions keep Cell.
use super::*;
use std::sync::{Arc, Mutex};
use zencodec::enough::Stop;

type Session = DecodeSession<Box<dyn Fn() -> bool + Send + Sync>>;
struct State {
    session: Session,
    failure: Option<Error>,
    frames: u32,
}
#[derive(Clone)]
pub(crate) struct Budget {
    session: Arc<Mutex<State>>,
    limits: zencodec::ResourceLimits,
}
impl Budget {
    pub(crate) fn new(
        policy: DecodePolicy,
        stop: Option<zencodec::StopToken>,
        limits: zencodec::ResourceLimits,
    ) -> Result<Self, Error> {
        // Check the fixed owner/cancellation allocation before constructing it.
        let fixed =
            size_of::<State>() + 2 * size_of::<usize>() + size_of::<Option<zencodec::StopToken>>();
        policy.reserve_working(fixed as u64)?;
        let session: Session = DecodeSession::with_cancel(
            policy,
            Box::new(move || stop.as_ref().is_some_and(|s| s.check().is_err())),
        );
        session.memory(fixed as u128)?;
        Ok(Self {
            limits,
            session: Arc::new(Mutex::new(State {
                session,
                failure: None,
                frames: 1,
            })),
        })
    }
    fn with<T>(&self, f: impl FnOnce(&Session) -> Result<T, Error>) -> Result<T, Error> {
        let mut state = self.session.lock().map_err(|_| Error::Cancelled)?;
        if let Some(ref e) = state.failure {
            return Err(e.clone());
        }
        let result = f(&state.session);
        if let Err(ref e) = result {
            state.failure = Some(e.clone());
        }
        result
    }
    pub(crate) fn failure(&self) -> Option<Error> {
        self.session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .failure
            .clone()
    }
    /// Admit a native canvas directly, without inventing a packed image description.
    #[cfg(feature = "lab")]
    pub(crate) fn canvas(&self, width: u32, height: u32) -> Result<(), Error> {
        self.limits
            .check_dimensions(width, height)
            .map_err(|_| Error::TooLarge)?;
        self.limits
            .check_total_pixels(u64::from(width) * u64::from(height))
            .map_err(|_| Error::TooLarge)?;
        self.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        self.with(|s| s.check())
    }
    pub(crate) fn capacity(&self, bytes: usize) -> Result<(), Error> {
        self.with(|s| {
            s.check()?;
            s.memory(bytes as u128)
        })
    }
    pub(crate) fn input(&self, bytes: usize) -> Result<(), Error> {
        self.with(|s| {
            s.check()?;
            let mut u = s.usage.get();
            u.input_bytes = add(u.input_bytes, bytes as u128, s.policy.input)?;
            self.limits
                .check_input_size(u.input_bytes)
                .map_err(|_| Error::TooLarge)?;
            s.work(bytes as u128)?;
            // work updates usage too.
            let mut next = s.usage.get();
            next.input_bytes = u.input_bytes;
            s.usage.set(next);
            Ok(())
        })
    }
    pub(crate) fn copy_work(&self, bytes: usize) -> Result<(), Error> {
        self.with(|s| s.work(bytes as u128))
    }
    pub(crate) fn header_bytes(&self, bytes: u64) -> Result<(), Error> {
        self.with(|s| {
            s.check()?;
            if bytes > s.policy.header {
                Err(Error::TooLarge)
            } else {
                Ok(())
            }
        })
    }
    pub(crate) fn usage(&self) -> DecodeUsage {
        self.session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .session
            .usage()
    }
}

macro_rules! forward {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $name(&self, $($arg: $ty),*) -> Result<(), Error> {
            self.with(|session| RequestAdmission { session, profile: ExecutionProfile::Rows }.$name($($arg),*))
        }
    )*};
}
impl Admission for Budget {
    fn v4_resume(&self) -> V4Resume {
        self.session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .session
            .v4_resume()
    }
    fn v4_commit(&self, state: V4Resume) -> Result<(), Error> {
        let mut owner = self.session.lock().map_err(|_| Error::Cancelled)?;
        let result = owner.session.v4_restore(state);
        if let Some(ref error) = owner.failure {
            return Err(error.clone());
        }
        if let Err(ref error) = result {
            owner.failure = Some(error.clone());
        }
        result
    }
    fn v4_cancel(&self) -> impl Fn() -> bool + Sync {
        let budget = self.clone();
        move || budget.with(|session| session.check()).is_err()
    }

    fn header(&self, info: &ImageInfo, bytes: usize) -> Result<(), Error> {
        self.limits
            .check_dimensions(info.width, info.height)
            .map_err(|_| Error::TooLarge)?;
        self.limits
            .check_total_pixels(u64::from(info.width) * u64::from(info.height))
            .map_err(|_| Error::TooLarge)?;
        self.limits.check_frames(1).map_err(|_| Error::TooLarge)?;
        self.with(|session| {
            RequestAdmission {
                session,
                profile: ExecutionProfile::Rows,
            }
            .header(info, bytes)
        })
    }

    fn chunk(&self, tag: [u8; 4], payload: &[u8]) -> Result<(), Error> {
        self.with(|session| {
            RequestAdmission {
                session,
                profile: ExecutionProfile::Rows,
            }
            .chunk(tag, payload)
        })?;
        if tag == *b"fRAM" {
            let mut state = self.session.lock().map_err(|_| Error::Cancelled)?;
            let next = state.frames.checked_add(1).ok_or(Error::TooLarge)?;
            if self.limits.check_frames(next).is_err() {
                state.failure = Some(Error::TooLarge);
                return Err(Error::TooLarge);
            }
            state.frames = next;
        }
        Ok(())
    }
    #[cfg(feature = "native-jpeg")]
    forward! { native(header: &crate::jpeg::NativeHeader, rows: u16); native_buffer(bytes: usize); }
    forward! {
        check();
        coding_profile(header: &crate::Header);

        chunk_start();

        model();
        local_ans(units:u128);
        ac_models(models:u128);
        lossless(info: &ImageInfo, layout: &crate::coding_layout::Layout, lanes: usize, rows: usize, group: usize, palette: bool);
        template(info: &ImageInfo, layout: &crate::coding_layout::Layout, lanes: usize, rows: usize, group: usize, palette: bool);
        lossy(info: &ImageInfo, rows: u8, group: usize, color: LossyColor, layouts: &[LossyChannelLayout; 3]);
        dc(parsed: &crate::v1::Parsed<'_>);
        exact(parsed: &crate::v1::Parsed<'_>);
        hint(bytes: usize);
        value_table(entries: usize);
        pixel_checksum(info: &ImageInfo);
    }
}
