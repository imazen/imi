//! Admission and cancellation for serial slice and synchronous row decoding.
//!
//! A session shares cumulative input, metadata, unit and work limits across
//! sequential calls. Working bytes are a conservative capacity plan, excluding
//! compressed input, output, allocator overhead and thread stacks; not RSS.
//! [`DecodeSession::decode_reader`] applies it to streamed input; the plain
//! [`crate::decode_reader`], frame and indexed-plane APIs do not inherit it. Cancellation
//! or malformed entropy after reconstruction starts can leave partial output.

#[cfg(feature = "std")]
pub(crate) mod feed;
mod rows;
pub(crate) use rows::AdmittedRows;

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::parse::limits::Admission;
use crate::{Error, ImageInfo, Limits, LossyChannelLayout, LossyColor};

pub(crate) const DEFAULT_METADATA_BYTES: usize = 16 << 20;

/// Limits for one serial request, possibly decoding multiple images.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct DecodePolicy {
    v4_profile: Option<crate::v4::CodingProfile>,
    structure_only: bool,
    group_pixels: bool,
    tile_pixels: bool,
    output: Limits,
    input: u64,
    header: u64,
    metadata: u64,
    chunks: u64,
    units: u64,
    work: u64,
    working: u64,
}

impl Default for DecodePolicy {
    fn default() -> Self {
        Self {
            v4_profile: None,
            structure_only: false,
            group_pixels: true,
            tile_pixels: true,
            output: Limits::DEFAULT,
            input: 1 << 30,
            header: 64 << 10,
            metadata: DEFAULT_METADATA_BYTES as u64,
            chunks: 65_536,
            units: 1_000_000,
            work: 8 << 30,
            working: 256 << 20,
        }
    }
}

impl DecodePolicy {
    /// Enforce a V4 coding profile on the images this session reads.
    /// The default accepts every supported version and coding.
    pub fn with_v4_profile(mut self, profile: crate::v4::CodingProfile) -> Self {
        self.v4_profile = Some(profile);
        self
    }

    pub(crate) fn with_optional_v4_profile(
        mut self,
        profile: Option<crate::v4::CodingProfile>,
    ) -> Self {
        self.v4_profile = profile;
        self
    }
    /// Select independent V4 pixel-check scopes. Both default to enabled.
    /// Disabling a scope keeps its framing/bounds checks, but does not establish
    /// pixel integrity for that scope. Encoded-object checks remain enabled.
    pub fn with_scoped_pixel_checks(mut self, groups: bool, tiles: bool) -> Self {
        self.group_pixels = groups;
        self.tile_pixels = tiles;
        self
    }

    // Encoded-domain writers/upgrades parse tables without selecting pixel kernels.
    pub(crate) fn for_structure(mut self) -> Self {
        self.structure_only = true;
        self
    }

    #[cfg(feature = "lab")]
    pub(crate) fn component_probe_limits(&self) -> (Limits, u64, u64, u64, u64, u64) {
        (
            self.output,
            self.input,
            self.header,
            self.metadata,
            self.chunks,
            self.working,
        )
    }

    pub(crate) fn output_limits(&self) -> Limits {
        self.output
    }
    pub(crate) fn check_probe(
        &self,
        info: &ImageInfo,
        header: usize,
        input: usize,
    ) -> Result<(), Error> {
        if header as u64 > self.header || input as u64 > self.input {
            return Err(Error::TooLarge);
        }
        crate::parse::limits::check_limits(info, &self.output)
    }

    pub(crate) fn cap_working(mut self, max: u64) -> Self {
        self.working = self.working.min(max);
        self
    }

    pub(crate) fn reserve_working(mut self, retained: u64) -> Result<Self, Error> {
        self.working = self.working.checked_sub(retained).ok_or(Error::TooLarge)?;
        Ok(self)
    }

    /// Output limits apply separately to each image, including caller-owned output.
    /// [`Limits::verify_checksum`] also sets whether a file checksum is verified.
    pub fn with_output_limits(mut self, limits: Limits) -> Self {
        self.output = limits;
        self
    }

    /// Cumulative supplied compressed bytes; failed calls retain prior charges.
    pub fn with_max_input_bytes(mut self, max: u64) -> Self {
        self.input = max;
        self
    }

    /// Maximum serialized fixed header bytes per image.
    pub fn with_max_header_bytes(mut self, max: u64) -> Self {
        self.header = max;
        self
    }

    /// Cumulative metadata payload allowance, including compressed expansion.
    pub fn with_max_metadata_bytes(mut self, max: u64) -> Self {
        self.metadata = max;
        self
    }

    /// Cumulative outer chunks, including ignored ancillary records.
    pub fn with_max_chunks(mut self, max: u64) -> Self {
        self.chunks = max;
        self
    }

    /// Cumulative stripe/group records, including nested DC and alpha.
    pub fn with_max_units(mut self, max: u64) -> Self {
        self.units = max;
        self
    }

    /// Cumulative padded sample/table/byte work allowance, not CPU instructions.
    pub fn with_max_work_units(mut self, max: u64) -> Self {
        self.work = max;
        self
    }

    /// Maximum planned live codec capacity per call; output is separately limited.
    pub fn with_max_working_bytes(mut self, max: u64) -> Self {
        self.working = max;
        self
    }
}

/// Admission counters, including successfully charged portions of failed calls.
/// Open DATA may visit models twice; both actual parsing visits are charged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeUsage {
    pub input_bytes: u64,
    pub chunks: u64,
    pub metadata_bytes: u64,
    pub units: u64,
    pub work_units: u64,
    pub peak_planned_working_bytes: u64,
}

/// Sequential decode request. `&mut self` excludes simultaneous use of a session.
/// Independent sessions have independent budgets; services must bound their count.
pub struct DecodeSession<C = fn() -> bool> {
    policy: DecodePolicy,
    cancel: Option<C>,
    usage: Cell<DecodeUsage>,
    working: Cell<u64>,
    v4_input_admitted: Cell<usize>,
    // Complete-input plans reserve reconstruction work before publishing pixels.
    // Jobs consume this allowance; live-input jobs continue charging as they run.
    v4_work_credit: Cell<u64>,
    #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
    v4_worker_budget: Option<Arc<V4WorkBudget>>,
    // Lazily allocated only for V4 objects that can outlive their loader.
    retained: RefCell<Option<Arc<AtomicUsize>>>,
    #[cfg(feature = "native-jpeg")]
    native_buffer: Cell<u64>,
}

// Workers draw from the same remaining work allowance. The count of queued
// groups does not affect admission, and no worker can overspend it.
#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
pub(crate) struct V4WorkBudget {
    limit: u64,
    spent: AtomicU64,
}

#[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
impl V4WorkBudget {
    fn charge(&self, amount: u128) -> Result<(), Error> {
        let mut spent = self.spent.load(Ordering::Relaxed);
        loop {
            let total = add(spent, amount, self.limit)?;
            match self.spent.compare_exchange_weak(
                spent,
                total,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => spent = actual,
            }
        }
    }
}

pub(crate) struct Inspection {
    pub info: ImageInfo,
    pub metadata: crate::Metadata,
    pub kind: crate::FrameKind,
    pub count: u64,
    pub loop_count: u32,
}

impl DecodeSession {
    pub fn new(policy: DecodePolicy) -> Self {
        Self::init(policy, None)
    }
}

impl<C: Fn() -> bool + Sync> DecodeSession<C> {
    pub(crate) fn v4_profile(&self) -> Option<crate::v4::CodingProfile> {
        self.policy.v4_profile
    }

    pub(crate) fn check_coding_profile(&self, header: &crate::Header) -> Result<(), Error> {
        self.policy
            .v4_profile
            .map_or(Ok(()), |p| p.check_header(header))
    }
    pub(crate) fn resume(policy: DecodePolicy, usage: DecodeUsage, cancel: C) -> Self {
        let session = Self::with_cancel(policy, cancel);
        session.usage.set(usage);
        session
    }

    pub(crate) fn admit_input(&self, data: &[u8]) -> Result<(), Error> {
        self.begin(data)
    }

    pub(crate) fn admission(&self) -> impl Admission + '_ {
        RequestAdmission {
            session: self,
            profile: ExecutionProfile::Serial,
        }
    }

    /// V4 numeric definitions contain tables, not image-wide unit schedules.
    pub(crate) fn v4_pixel_checks(&self, scope: crate::v4::CheckScope) -> bool {
        match scope {
            crate::v4::CheckScope::GroupPixels => self.policy.group_pixels,
            crate::v4::CheckScope::TilePixels => self.policy.tile_pixels,
            crate::v4::CheckScope::EncodedObject => true,
        }
    }

    pub(crate) fn v4_decodes(&self) -> bool {
        !self.policy.structure_only
    }
    pub(crate) fn v4_models(&self) -> impl Admission + '_ {
        RequestAdmission {
            session: self,
            profile: ExecutionProfile::Models,
        }
    }

    // Geometric descriptor growth, admitting capacity and old/new coexistence.
    // Charge worst-case copy work so tiny-group files cannot hide quadratic
    // reallocation behind a per-record work allowance.
    pub(crate) fn v4_reserve<T>(
        &self,
        values: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), Error> {
        let needed = values
            .len()
            .checked_add(additional)
            .ok_or(Error::TooLarge)?;
        let old = values.capacity();
        if needed <= old {
            return Ok(());
        }
        let capacity = needed.max(old.saturating_mul(2)).max(8);
        let bytes = capacity
            .checked_sub(old)
            .and_then(|n| n.checked_mul(core::mem::size_of::<T>()))
            .ok_or(Error::TooLarge)?;
        self.v4_models().hint(bytes)?;
        let _peak = self.v4_working(old as u128 * core::mem::size_of::<T>() as u128)?;
        self.adapter_work(values.len() as u128 * core::mem::size_of::<T>() as u128)?;
        values
            .try_reserve_exact(capacity - values.len())
            .map_err(|_| Error::TooLarge)
    }

    pub(crate) fn v4_model_scope(&self) -> V4ModelScope<'_, C> {
        V4ModelScope {
            session: self,
            base: self.working.get(),
            active: true,
        }
    }

    pub(crate) fn v4_header_limit(&self) -> u64 {
        self.policy.header
    }

    pub(crate) fn v4_check_input(&self, bytes: usize) -> Result<(), Error> {
        self.check()?;
        if self.v4_input_admitted.get() != 0 {
            return Ok(());
        }
        add(
            self.usage.get().input_bytes,
            bytes as u128,
            self.policy.input,
        )
        .map(|_| ())
    }

    pub(crate) fn v4_verify_mode(&self) -> crate::checksum::Verify {
        if self.policy.output.verify_checksum {
            crate::checksum::Verify::Defer
        } else {
            crate::checksum::Verify::Off
        }
    }

    pub(crate) fn v4_precharged(&self) -> Result<V4AdmittedInput<'_, C>, Error> {
        self.v4_input_admitted.set(
            self.v4_input_admitted
                .get()
                .checked_add(1)
                .ok_or(Error::TooLarge)?,
        );
        Ok(V4AdmittedInput(self))
    }

    pub(crate) fn v4_working(&self, bytes: u128) -> Result<V4Working<'_, C>, Error> {
        self.check()?;
        self.memory(bytes)?;
        Ok(V4Working {
            session: self,
            bytes: bytes as u64,
        })
    }

    #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
    pub(crate) fn v4_worker_policy(
        &self,
        buffers: u128,
        workers: usize,
    ) -> Result<(DecodePolicy, u64), Error> {
        self.check()?;
        let retained = self
            .retained
            .borrow()
            .as_ref()
            .map_or(0, |n| n.load(Ordering::Relaxed)) as u64;
        let available = self
            .policy
            .working
            .min(isize::MAX as u64)
            .checked_sub(self.working.get())
            .and_then(|n| n.checked_sub(retained))
            .ok_or(Error::TooLarge)?;
        let scratch = u128::from(available)
            .checked_sub(buffers)
            .ok_or(Error::TooLarge)?;
        let count = u64::try_from(workers).map_err(|_| Error::TooLarge)?;
        if count == 0 {
            return Err(Error::InvalidInput("V4 worker count"));
        }
        let policy = self
            .policy
            .with_max_input_bytes(0)
            .with_max_working_bytes((scratch / u128::from(count)) as u64)
            .with_max_work_units(self.policy.work.saturating_sub(self.usage.get().work_units));
        // Reserve the whole remaining allowance while the independent sessions
        // are live. Their per-worker limits partition that allowance, including
        // temporary local output. This is capacity admission, not measured RSS.
        Ok((policy, available))
    }

    #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
    pub(crate) fn v4_work_budget(policy: DecodePolicy) -> Arc<V4WorkBudget> {
        Arc::new(V4WorkBudget {
            limit: policy.work,
            spent: AtomicU64::new(0),
        })
    }

    #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
    pub(crate) fn v4_worker(policy: DecodePolicy, cancel: C, budget: &Arc<V4WorkBudget>) -> Self {
        let mut worker = Self::with_cancel(policy, cancel);
        worker.v4_worker_budget = Some(Arc::clone(budget));
        worker
    }

    #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
    pub(crate) fn v4_charge_workers(&self, budget: &V4WorkBudget) -> Result<(), Error> {
        self.charge_work(u128::from(budget.spent.load(Ordering::Relaxed)))
    }

    pub(crate) fn v4_cancel(&self) -> Option<&C> {
        self.cancel.as_ref()
    }

    pub(crate) fn v4_restore(&self, state: V4Resume) -> Result<(), Error> {
        self.usage.set(state.usage);
        self.working.set(state.working);
        self.v4_work_credit.set(state.work_credit);
        *self.retained.borrow_mut() = state.retained;
        self.check()
    }

    pub(crate) fn v4_resume(&self) -> V4Resume {
        V4Resume {
            policy: self.policy,
            usage: self.usage.get(),
            working: self.working.get(),
            work_credit: self.v4_work_credit.get(),
            retained: self.retained.borrow().clone(),
        }
    }

    pub(crate) fn adapter_work(&self, bytes: u128) -> Result<(), Error> {
        self.work(bytes)
    }

    pub(crate) fn v4_reserve_work(&self, work: u128) -> Result<(), Error> {
        self.check()?;
        let credit = add(self.v4_work_credit.get(), work, u64::MAX)?;
        self.charge_work(work)?;
        self.v4_work_credit.set(credit);
        Ok(())
    }

    /// V4 framed-object input, charged once when delivered by its object reader.
    /// This does not reset the request or refund failed/corrupt objects.
    pub(crate) fn v4_object(&self, bytes: usize, unit: bool) -> Result<(), Error> {
        self.v4_input(bytes)?;
        self.v4_object_admitted(bytes, unit)
    }

    pub(crate) fn v4_object_admitted(&self, bytes: usize, unit: bool) -> Result<(), Error> {
        let mut usage = self.usage.get();
        if unit {
            usage.units = add(usage.units, 1, self.policy.units)?;
        }
        self.usage.set(usage);
        self.work(bytes as u128 + 1)
    }

    pub(crate) fn v4_input(&self, bytes: usize) -> Result<(), Error> {
        self.check()?;
        if self.v4_input_admitted.get() != 0 {
            return Ok(());
        }
        let mut usage = self.usage.get();
        usage.input_bytes = add(usage.input_bytes, bytes as u128, self.policy.input)?;
        self.usage.set(usage);
        Ok(())
    }

    pub(crate) fn v4_output(&self, info: &ImageInfo) -> Result<(), Error> {
        self.check()?;
        crate::parse::limits::check_limits(info, &self.policy.output)
    }
    /// Check a known whole-request lower bound without charging future input.
    /// Sequential readers can reject impossible group budgets from setup while
    /// retaining incremental accounting for objects that have actually arrived.
    #[cfg(feature = "std")]
    pub(crate) fn v4_check_units(&self, count: u64) -> Result<(), Error> {
        self.check()?;
        add(self.usage.get().units, u128::from(count), self.policy.units).map(|_| ())
    }

    pub(crate) fn v4_setup(&self, bytes: usize) -> Result<(), Error> {
        self.v4_input(bytes)?;
        self.work(bytes as u128 + 1)
    }

    pub(crate) fn v4_setup_admitted(&self, bytes: usize) -> Result<(), Error> {
        self.work(bytes as u128 + 1)
    }

    /// `cancel` returns true to stop. It is checked at parser and decode-unit
    /// boundaries, including rows of nested lossless images. Supported bitmap
    /// checksum verification also checks every 64 KiB and before success.
    /// No clock is required.
    pub fn with_cancel(policy: DecodePolicy, cancel: C) -> Self {
        Self::init(policy, Some(cancel))
    }

    fn init(policy: DecodePolicy, cancel: Option<C>) -> Self {
        Self {
            policy,
            cancel,
            usage: Cell::new(DecodeUsage::default()),
            working: Cell::new(0),
            v4_input_admitted: Cell::new(0),
            v4_work_credit: Cell::new(0),
            #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
            v4_worker_budget: None,
            retained: RefCell::new(None),
            #[cfg(feature = "native-jpeg")]
            native_buffer: Cell::new(0),
        }
    }

    pub fn usage(&self) -> DecodeUsage {
        self.usage.get()
    }

    fn begin(&self, data: &[u8]) -> Result<(), Error> {
        self.working.set(0);
        self.v4_work_credit.set(0);
        #[cfg(feature = "native-jpeg")]
        self.native_buffer.set(0);
        self.check()?;
        let mut usage = self.usage.get();
        usage.input_bytes = add(usage.input_bytes, data.len() as u128, self.policy.input)?;
        self.usage.set(usage);
        self.work(data.len() as u128)
    }

    /// Adapter metadata inspection uses the same pre-expansion admission as decode.
    pub(crate) fn inspect(
        &mut self,
        data: &[u8],
        keep: impl Fn([u8; 4]) -> Result<bool, Error>,
    ) -> Result<Inspection, Error> {
        self.begin(data)?;
        let f = if crate::parse::header::header_version(data)? == 4 {
            let _input = self.v4_precharged()?;
            crate::v4::file::parse_file(data, self)?
        } else {
            crate::v1::parse_file(
                data,
                &self.admission(),
                crate::checksum::Verify::from_flag(self.policy.output.verify_checksum, 1),
            )?
        };
        let (kind, loop_count, count, _) = crate::frames::sequence_control(&f)?;
        let mut meta = crate::Metadata {
            orientation: f.header.orientation,
            ..Default::default()
        };
        for &(tag, payload) in &f.metadata {
            self.check()?;
            let logical = if tag == *b"mCMP" {
                payload
                    .get(..4)
                    .ok_or(Error::Corrupt("short compressed metadata"))?
                    .try_into()
                    .unwrap()
            } else {
                tag
            };
            if keep(logical)? {
                crate::container::keep_metadata(&mut meta, tag, payload)?;
            }
        }
        self.check()?;
        Ok(Inspection {
            info: f.header.info,
            metadata: meta,
            kind,
            count,
            loop_count,
        })
    }

    /// Decode primary pixels into a new tightly packed, native-endian buffer.
    pub fn decode(&mut self, data: &[u8]) -> Result<(ImageInfo, Vec<u8>), Error> {
        self.begin(data)?;
        if crate::parse::header::header_version(data)? == 4 {
            let _input = self.v4_precharged()?;
            return crate::v4::decode::allocating(data, self);
        }
        let admission = RequestAdmission {
            session: self,
            profile: ExecutionProfile::Serial,
        };
        let parsed = prepare(data, &admission, self.policy.output.verify_checksum)?;
        let info = parsed.0.info();
        let size = info.checked_buffer_len().ok_or(Error::TooLarge)?;
        let stride = info.row_bytes();
        self.check()?;
        let mut out = Vec::new();
        out.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
        out.resize(size, 0);
        let table = parsed.1;
        let info = parsed.0.decode(&mut out, stride, self.cancel.as_ref())?;
        table.verify_checked(0, &info, &out, stride, || self.check())?;
        Ok((info, out))
    }

    /// Decode primary pixels into caller storage. Stride is in bytes. Budget
    /// admission completes before output mutation; later errors are not atomic.
    pub fn decode_into(
        &mut self,
        data: &[u8],
        out: &mut [u8],
        stride: usize,
    ) -> Result<ImageInfo, Error> {
        self.begin(data)?;
        if crate::parse::header::header_version(data)? == 4 {
            let _input = self.v4_precharged()?;
            return crate::v4::decode::into(data, out, stride, self).map(|(header, _)| header.info);
        }
        let admission = RequestAdmission {
            session: self,
            profile: ExecutionProfile::Serial,
        };
        let parsed = prepare(data, &admission, self.policy.output.verify_checksum)?;
        let extent = parsed
            .0
            .info()
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("invalid output layout"))?;
        if out.len() < extent {
            return Err(Error::InvalidInput("output buffer too small"));
        }
        self.check()?;
        let table = parsed.1;
        let info = parsed.0.decode(out, stride, self.cancel.as_ref())?;
        table.verify_checked(0, &info, out, stride, || self.check())?;
        Ok(info)
    }

    /// [`Self::decode_into`] reporting each lossless unit to a live `pulse`.
    pub(crate) fn decode_into_observed(
        &mut self,
        data: &[u8],
        out: &mut [u8],
        stride: usize,
        pulse: &dyn how_far::Pulse,
    ) -> Result<ImageInfo, Error> {
        self.begin(data)?;
        if crate::parse::header::header_version(data)? == 4 {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let _input = self.v4_precharged()?;
            return crate::v4::decode::observed(data, out, stride, self, Some(pulse))
                .map(|(header, _)| header.info);
        }
        let admission = RequestAdmission {
            session: self,
            profile: ExecutionProfile::Serial,
        };
        let parsed = prepare(data, &admission, self.policy.output.verify_checksum)?;
        let extent = parsed
            .0
            .info()
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("invalid output layout"))?;
        if out.len() < extent {
            return Err(Error::InvalidInput("output buffer too small"));
        }
        self.check()?;
        let table = parsed.1;
        let info = parsed
            .0
            .decode_observed(out, stride, self.cancel.as_ref(), pulse)?;
        table.verify_checked(0, &info, out, stride, || self.check())?;
        Ok(info)
    }

    /// Decode primary pixels one tightly packed native-endian row at a time.
    /// `on_row` receives zero-based indices and only complete rows. Extra caller
    /// storage is untouched. Full image output limits still apply. The complete
    /// slice, including suffix chunks, is validated before the first callback.
    /// Callback errors or cancellation abort without refunding admitted work;
    /// the caller row may be partially written on a late error. Callback work
    /// and allocations belong to the application and are outside this policy.
    pub fn decode_rows<F>(
        &mut self,
        data: &[u8],
        row: &mut [u8],
        mut on_row: F,
    ) -> Result<ImageInfo, Error>
    where
        F: FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
    {
        self.begin(data)?;
        if crate::parse::header::header_version(data)? == 4 {
            let mut state = self.v4_row_state(alloc::borrow::Cow::Borrowed(data))?;
            let info = state.info().clone();
            let cancel = self.cancel.as_ref();
            let check = || {
                if cancel.is_some_and(|c| c()) {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            };
            let result = (|| {
                for y in 0..info.height {
                    if !state.next(row, &check)? {
                        return Err(Error::Corrupt("V4 primary image too short"));
                    }
                    on_row(&info, y, &row[..info.row_bytes()])?;
                }
                self.check()?;
                Ok(info)
            })();
            if let Some(usage) = state.usage() {
                self.usage.set(usage);
            }
            return result;
        }

        let admission = RequestAdmission {
            session: self,
            profile: ExecutionProfile::Rows,
        };
        let parsed = prepare(data, &admission, self.policy.output.verify_checksum)?;
        if row.len() < parsed.0.info().row_bytes() {
            return Err(Error::InvalidInput("row buffer too small"));
        }
        self.check()?;
        let mut pixel_check = crate::pixelsum::RowCheck::new(parsed.1, parsed.0.info())?;
        let info = parsed.0.decode_rows(
            row,
            |info, y, row| {
                if let Some(c) = &mut pixel_check {
                    c.row_checked(row, || self.check())?;
                }
                on_row(info, y, row)
            },
            self.cancel.as_ref(),
        )?;
        if pixel_check.is_some() {
            self.check()?;
        }
        Ok(info)
    }

    /// Decode a file streamed from `reader`, one tightly packed native-endian
    /// row at a time, under this session's policy. Returns the image info and
    /// metadata as [`crate::decode_reader`] does.
    ///
    /// Unlike the slice methods, input is charged as it is delivered: the
    /// call may consume at most the remaining cumulative input allowance, and
    /// the bytes it consumed stay charged when it fails. Headers, every
    /// chunk, retained metadata (after its bytes arrive), models, records
    /// and planned working memory are admitted from the parsed heads before
    /// the corresponding buffers grow; cancellation is checked at parser,
    /// stripe and row boundaries. Rows are delivered as they decode, so a
    /// later error can follow callbacks; trailing chunks are validated after
    /// the last row. The policy's output limits replace per-call [`Limits`].
    #[cfg(feature = "std")]
    pub fn decode_reader<R, F>(
        &mut self,
        reader: R,
        mut on_row: F,
    ) -> Result<(ImageInfo, crate::Metadata), Error>
    where
        R: std::io::Read,
        F: FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
    {
        self.decode_reader_rows(reader, &mut on_row)
    }

    #[cfg(feature = "std")]
    pub(crate) fn decode_reader_rows<R: std::io::Read>(
        &mut self,
        reader: R,
        on_row: &mut (impl crate::read::input::Rows + ?Sized),
    ) -> Result<(ImageInfo, crate::Metadata), Error> {
        self.working.set(0);
        #[cfg(feature = "native-jpeg")]
        self.native_buffer.set(0);
        self.check()?;
        let used = self.usage.get().input_bytes;
        let budget = self.policy.input.saturating_sub(used);
        let admission = RequestAdmission {
            session: &*self,
            profile: ExecutionProfile::Rows,
        };
        let (result, consumed) = crate::read::decode_reader_rows_with(
            reader,
            &self.policy.output,
            budget,
            &admission,
            on_row,
        );
        let mut usage = self.usage.get();
        usage.input_bytes = used.saturating_add(consumed);
        self.usage.set(usage);
        let charged = self.work(u128::from(consumed));
        let out = result?;
        charged?;
        Ok(out)
    }

    #[cfg(feature = "std")]
    pub(crate) fn inspect_payload_head(
        &mut self,
        header: &crate::parse::header::ParsedHeader,
        input: &mut impl crate::parse::payload::HeadInput,
    ) -> Result<crate::parse::payload::Head, Error> {
        self.working.set(0);
        #[cfg(feature = "native-jpeg")]
        self.native_buffer.set(0);
        let admission = RequestAdmission {
            session: &*self,
            profile: ExecutionProfile::Rows,
        };
        admission.header(&header.header.info, header.len)?;
        crate::parse::payload::read_head(&header.header, header.declared, input, &admission)
    }

    fn check(&self) -> Result<(), Error> {
        if self.cancel.as_ref().is_some_and(|cancel| cancel()) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    fn memory(&self, bytes: u128) -> Result<(), Error> {
        let retained = self
            .retained
            .borrow()
            .as_ref()
            .map_or(0, |n| n.load(Ordering::Relaxed)) as u64;
        let limit = self.policy.working.min(isize::MAX as u64);
        let next = add(self.working.get(), bytes, limit)?;
        let total = add(next, retained as u128, limit)?;
        self.working.set(next);
        let mut usage = self.usage.get();
        usage.peak_planned_working_bytes = usage.peak_planned_working_bytes.max(total);
        self.usage.set(usage);
        Ok(())
    }

    fn work(&self, amount: u128) -> Result<(), Error> {
        self.check()?;
        let credit = u128::from(self.v4_work_credit.get());
        let prepaid = amount.min(credit);
        self.v4_work_credit.set((credit - prepaid) as u64);
        self.charge_work(amount - prepaid)
    }

    fn charge_work(&self, amount: u128) -> Result<(), Error> {
        let mut usage = self.usage.get();
        usage.work_units = add(usage.work_units, amount, self.policy.work)?;
        #[cfg(all(feature = "std", any(not(target_family = "wasm"), feature = "rayon")))]
        if amount != 0
            && let Some(budget) = &self.v4_worker_budget
        {
            budget.charge(amount)?;
        }
        self.usage.set(usage);
        Ok(())
    }
}

fn add(current: u64, amount: u128, limit: u64) -> Result<u64, Error> {
    let total = u128::from(current)
        .checked_add(amount)
        .ok_or(Error::TooLarge)?;
    if total > u128::from(limit) {
        return Err(Error::TooLarge);
    }
    Ok(total as u64)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecutionProfile {
    Serial,
    Rows,
    Models,
}

/// Transfer the same live retained account across suspended row/feed calls.
/// Counter snapshots alone would forget capacity owned by queued models/jobs.
pub(crate) struct V4Resume {
    policy: DecodePolicy,
    usage: DecodeUsage,
    working: u64,
    work_credit: u64,
    retained: Option<Arc<AtomicUsize>>,
}
impl V4Resume {
    pub(crate) fn for_structure(&mut self) {
        self.policy = self.policy.for_structure();
    }

    pub(crate) fn header_limit(&self) -> u64 {
        self.policy.header
    }
    pub(crate) fn nested_policy(&mut self, verify: bool, limits: Option<Limits>) {
        self.policy.output.verify_checksum = verify;
        if let Some(limits) = limits {
            self.policy.output.max_pixels = self.policy.output.max_pixels.min(limits.max_pixels);
            self.policy.output.max_bytes = self.policy.output.max_bytes.min(limits.max_bytes);
        }
    }
    pub(crate) fn session<C: Fn() -> bool + Sync>(&self, cancel: C) -> DecodeSession<C> {
        let session = DecodeSession::resume(self.policy, self.usage, cancel);
        session.working.set(self.working);
        session.v4_work_credit.set(self.work_credit);
        *session.retained.borrow_mut() = self.retained.clone();
        session
    }
    /// A positional reader's next region has fresh work/input counters but
    /// shares capacity still owned by retained tables and index state.
    pub(crate) fn request<C: Fn() -> bool + Sync>(
        &self,
        mut policy: DecodePolicy,
        cancel: C,
    ) -> DecodeSession<C> {
        policy.v4_profile = self.policy.v4_profile;
        let session = DecodeSession::with_cancel(policy, cancel);
        session.working.set(self.working);
        *session.retained.borrow_mut() = self.retained.clone();
        session
    }
    pub(crate) fn update<C: Fn() -> bool + Sync>(&mut self, session: &DecodeSession<C>) {
        self.usage = session.usage();
        self.retained = session.retained.borrow().clone();
        self.working = session.working.get();
        self.work_credit = session.v4_work_credit.get();
    }
    pub(crate) fn usage(&self) -> DecodeUsage {
        self.usage
    }
}

/// Temporary V4 working capacity; output and retained model tables are separate.
pub(crate) struct V4Working<'s, C> {
    session: &'s DecodeSession<C>,
    bytes: u64,
}

impl<C: Fn() -> bool + Sync> V4Working<'_, C> {
    pub(crate) fn retain(mut self) -> Result<V4Retained, Error> {
        self.session.check()?;
        let bytes = usize::try_from(self.bytes).map_err(|_| Error::TooLarge)?;
        let account = if bytes == 0 {
            None
        } else {
            let mut retained = self.session.retained.borrow_mut();
            let account = retained.get_or_insert_with(|| Arc::new(AtomicUsize::new(0)));
            account.fetch_add(bytes, Ordering::Relaxed);
            Some(Arc::clone(account))
        };
        self.session.working.set(
            self.session
                .working
                .get()
                .checked_sub(self.bytes)
                .ok_or(Error::TooLarge)?,
        );
        self.bytes = 0;
        Ok(V4Retained { account, bytes })
    }
}

/// Model storage follows the final shared owner across workers. It contains
/// no borrow of the (single-threaded) decode session. Atomic pointer width is
/// sufficient because every working admission is bounded by isize::MAX.
#[derive(Default)]
pub(crate) struct V4Retained {
    account: Option<Arc<AtomicUsize>>,
    bytes: usize,
}

impl Drop for V4Retained {
    fn drop(&mut self) {
        if let Some(account) = &self.account {
            let previous = account.fetch_sub(self.bytes, Ordering::Relaxed);
            debug_assert!(previous >= self.bytes);
        }
    }
}

/// Parsing scopes turn their admitted owned capacities into a movable charge.
/// Failed parses refund live capacity; actual input/work counters remain used.
pub(crate) struct V4ModelScope<'s, C> {
    session: &'s DecodeSession<C>,
    base: u64,
    active: bool,
}

impl<C> V4ModelScope<'_, C> {
    pub(crate) fn commit(mut self) -> Result<V4Retained, Error> {
        let bytes = usize::try_from(
            self.session
                .working
                .get()
                .checked_sub(self.base)
                .ok_or(Error::Corrupt("unbalanced V4 model admission"))?,
        )
        .map_err(|_| Error::TooLarge)?;
        let account = if bytes != 0 {
            let mut retained = self.session.retained.borrow_mut();
            let account = retained.get_or_insert_with(|| Arc::new(AtomicUsize::new(0)));
            account.fetch_add(bytes, Ordering::Relaxed);
            Some(Arc::clone(account))
        } else {
            None
        };
        self.session.working.set(self.base);
        self.active = false;
        Ok(V4Retained { account, bytes })
    }
}

impl<C> Drop for V4ModelScope<'_, C> {
    fn drop(&mut self) {
        if self.active {
            self.session.working.set(self.base);
        }
    }
}

impl<C> Drop for V4Working<'_, C> {
    fn drop(&mut self) {
        self.session
            .working
            .set(self.session.working.get() - self.bytes);
    }
}

struct RequestAdmission<'s, C> {
    session: &'s DecodeSession<C>,
    profile: ExecutionProfile,
}

impl<C: Fn() -> bool + Sync> Admission for RequestAdmission<'_, C> {
    fn decodes(&self) -> bool {
        self.session.v4_decodes()
    }

    fn v4_resume(&self) -> V4Resume {
        self.session.v4_resume()
    }
    fn v4_commit(&self, state: V4Resume) -> Result<(), Error> {
        self.session.v4_restore(state)
    }
    fn v4_cancel(&self) -> impl Fn() -> bool + Sync {
        let cancel = self.session.cancel.as_ref();
        move || cancel.is_some_and(|c| c())
    }

    fn check(&self) -> Result<(), Error> {
        self.session.check()
    }

    fn header(&self, info: &ImageInfo, bytes: usize) -> Result<(), Error> {
        self.check()?;
        if bytes as u64 > self.session.policy.header {
            return Err(Error::TooLarge);
        }
        if self.profile != ExecutionProfile::Models {
            crate::parse::limits::check_limits(info, &self.session.policy.output)?;
        }
        self.session.memory(16 * 1024)
    }

    fn coding_profile(&self, header: &crate::Header) -> Result<(), Error> {
        self.session.check_coding_profile(header)
    }

    fn chunk_start(&self) -> Result<(), Error> {
        self.check()?;
        let mut usage = self.session.usage.get();
        usage.chunks = add(usage.chunks, 1, self.session.policy.chunks)?;
        self.session.usage.set(usage);
        // Borrowed lists/unknown chunk descriptors, including Vec growth.
        self.session.memory(128)
    }

    fn chunk(&self, tag: [u8; 4], payload: &[u8]) -> Result<(), Error> {
        self.check()?;
        if tag == *b"aNIM"
            && let Some(profile) = self.session.v4_profile()
        {
            profile.check_sequence()?;
        }
        let retained = match &tag {
            b"DATA" | b"uIDX" | b"pRVW" | b"aNIM" | b"fRAM" | b"pDAT" | b"pIDX" | b"sLOC"
            | b"cSUM" | b"pSUM" => 0,
            b"mCMP" => crate::metadata_pack::unpacked_len(
                payload
                    .get(4..)
                    .ok_or(Error::Corrupt("short compressed metadata"))?,
            )? as u128,
            // Invalid gain-map parsing may temporarily copy before fallback.
            b"gMAP" => 2 * payload.len() as u128,
            _ => payload.len() as u128,
        };
        let mut usage = self.session.usage.get();
        usage.metadata_bytes = add(usage.metadata_bytes, retained, self.session.policy.metadata)?;
        self.session.usage.set(usage);
        self.session.memory(retained)?;
        self.session.work(retained + 1)
    }

    fn model(&self) -> Result<(), Error> {
        self.check()?;
        self.session.memory(16 * 1024)
    }

    fn quick(&self, info: &ImageInfo, rows: usize, group: usize, bits: bool) -> Result<(), Error> {
        self.check()?;
        if self.profile == ExecutionProfile::Models {
            return Ok(());
        }
        let w = u128::from(info.width);
        let h = u128::from(info.height);
        let gw = if group == 0 { w } else { group as u128 };
        let groups = w.div_ceil(gw);
        let units = groups * h.div_ceil((rows as u128).min(h));
        let states = if self.profile == ExecutionProfile::Rows {
            groups
        } else if bits {
            // The header hook does not know the requested worker ceiling.
            // Charging every unit is conservative for any parallel schedule.
            units
        } else {
            1
        };
        self.session.memory(
            states
                * (crate::quick::state_bytes(info, gw, bits)?
                    + gw * info.bytes_per_pixel() as u128)
                + 32 * units
                + 2 * groups * crate::v1::stream_group_descriptor_bytes() as u128,
        )?;
        self.session.units(units)?;
        self.session.work(
            w * h * (1 + info.bytes_per_pixel() as u128)
                + if bits {
                    w * h * info.bytes_per_pixel() as u128 * 16
                        + units * info.bytes_per_pixel() as u128 * 2048
                } else {
                    0
                },
        )
    }

    fn lossless(
        &self,
        info: &ImageInfo,
        layout: &crate::coding_layout::Layout,
        lanes: usize,
        rows: usize,
        group: usize,
        palette: bool,
    ) -> Result<(), Error> {
        let w = u128::from(info.width);
        let h = u128::from(info.height);
        let gw = if group == 0 { w } else { group as u128 };
        let groups = w.div_ceil(gw);
        let rows = (rows as u128).min(h);
        let units = groups * h.div_ceil(rows);
        let slots = layout.slot_channel.len() as u128;
        let physical = gw.div_ceil(lanes as u128) * 16;
        let sample = if layout.slot_bits.iter().all(|&b| b <= 16) {
            2
        } else {
            4
        };
        let lut: u128 = layout
            .maps
            .iter()
            .map(|m| {
                crate::v1::OutMap::lut_entries(m, info.sample.word_depth()) as u128
                    * size_of::<u32>() as u128
            })
            .sum();
        let models = slots + u128::from(palette);
        let tables = models * 24 * 1024 + lut;
        if self.profile == ExecutionProfile::Models {
            self.session.memory(tables)?;
            return self.session.work(models * 65536);
        }
        let state = crate::decoder_memory::lossless_state_bytes(
            physical,
            slots,
            sample,
            rows,
            palette,
            info.bytes_per_pixel() as u128,
            gw,
        )?;
        let states = if self.profile == ExecutionProfile::Rows {
            groups
        } else {
            1
        };
        let descriptors = if self.profile == ExecutionProfile::Rows {
            2 * groups * crate::v1::stream_group_descriptor_bytes() as u128
                + crate::v1::stream_record_extra_bytes() as u128
        } else {
            0
        };
        self.session
            .memory(tables + states * state + descriptors + 32 * units)?;
        self.session.units(units)?;
        self.session
            .work(physical * groups * h * (slots + 1) + models * 65536)
    }

    fn template(
        &self,
        info: &ImageInfo,
        layout: &crate::coding_layout::Layout,
        _lanes: usize,
        _rows: usize,
        group: usize,
        palette: bool,
    ) -> Result<(), Error> {
        if self.profile == ExecutionProfile::Models {
            self.session.memory(crate::template::TABLE_BYTES as u128)?;
            return self.session.work(65536);
        }
        let w = u128::from(info.width);
        let h = u128::from(info.height);
        let gw = if group == 0 { w } else { group as u128 };
        let groups = w.div_ceil(gw);
        let state = crate::decoder_memory::template_state_bytes(
            layout.slot_channel.len() as u128,
            palette,
            info.bytes_per_pixel() as u128,
            gw,
        )?;
        let states = if self.profile == ExecutionProfile::Rows {
            groups
        } else {
            1
        };
        self.session
            .memory(crate::template::TABLE_BYTES as u128 + states * state)?;
        // Up to seven symbols per pixel beyond the ordinary charge's one,
        // and the tables' construction.
        self.session.work(w * h * 6 + 65536)
    }

    fn local_ans(&self, units: u128) -> Result<(), Error> {
        // The serial full-image decoder may retain two independent
        // inline models while decoding a paired stripe. Row decoding uses
        // at most one; the pair allowance is conservative there.
        self.session
            .memory(2 * crate::lossy::decoder_table_bytes())?;
        self.session
            .work(units * crate::lossy::decoder_table_bytes())
    }
    fn ac_models(&self, models: u128) -> Result<(), Error> {
        self.session
            .memory(models * crate::lossy::decoder_table_bytes())?;
        self.session
            .work(models * crate::lossy::decoder_table_bytes())
    }

    fn lossy(
        &self,
        info: &ImageInfo,
        rows: u8,
        group: usize,
        color: LossyColor,
        layouts: &[LossyChannelLayout; 3],
    ) -> Result<(), Error> {
        if self.profile == ExecutionProfile::Models {
            let tables = crate::lossy::decoder_table_bytes()
                + crate::lossy::decoder_transfer_table_bytes(info, color);
            self.session.memory(tables)?;
            return self.session.work(tables);
        }
        let w = u128::from(info.width);
        let h = u128::from(info.height);
        let gw = if group == 0 { w } else { group as u128 };
        let groups = w.div_ceil(gw);
        let block_rows = h.div_ceil(8).min(u128::from(rows));
        let units = groups * h.div_ceil(8).div_ceil(u128::from(rows));
        // Paired coefficient/EOB buffers, growth and split-Huffman side info.
        let coefficients = crate::decoder_memory::lossy_stripe_bytes(gw, block_rows)?;
        let staged = 2 * gw * (block_rows * 8).min(h) * 4 * info.sample.bytes() as u128;
        let scheduler = 64 * h * groups + 512 * units;
        let stack = if layouts != &[LossyChannelLayout::default(); 3] {
            16 * 1024
        } else {
            0
        };
        let row_staging = if self.profile == ExecutionProfile::Rows {
            crate::decoder_memory::row_stream_bytes(
                w,
                h,
                gw,
                block_rows,
                info.sample.bytes() as u128,
            )?
        } else {
            0
        };
        self.session.memory(
            crate::lossy::decoder_table_bytes()
                + crate::lossy::decoder_transfer_table_bytes(info, color)
                + crate::lossy::decoder_recon_bytes()
                + coefficients
                + staged
                + scheduler
                + stack
                + 32 * units
                + 32 * gw.div_ceil(8)
                + row_staging
                + 32 * 1024,
        )?;
        self.session.units(units)?;
        self.session.work(
            gw.div_ceil(8) * groups * h.div_ceil(8) * 64 * 3 + crate::lossy::decoder_table_bytes(),
        )
    }

    fn dc(&self, parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        if self.profile == ExecutionProfile::Models {
            return self.check();
        }
        // Actual nested height, not the encoder's usual eight-block grouping.
        let width = if self.profile == ExecutionProfile::Rows || parsed.gcols == 0 {
            parsed.info.width as u128
        } else {
            parsed.gcols as u128
        };
        let rows = (parsed.hs as u128).min(parsed.info.height as u128);
        self.session
            .memory(2 * width * rows * (32 + 16 + parsed.bpp() as u128))
    }

    #[cfg(feature = "native-jpeg")]
    fn native(&self, header: &crate::jpeg::NativeHeader, rows: u16) -> Result<(), Error> {
        self.check()?;
        let (memory, units, work) = header.plan(rows)?;
        self.session.memory(memory)?;
        self.session.units(units)?;
        self.session.work(work)
    }
    #[cfg(feature = "std")]
    #[cfg(feature = "native-jpeg")]
    fn native_buffer(&self, bytes: usize) -> Result<(), Error> {
        self.check()?;
        let planned = (bytes as u64).checked_mul(2).ok_or(Error::TooLarge)?;
        let previous = self.session.native_buffer.get();
        if planned > previous {
            self.session.memory((planned - previous) as u128)?;
            self.session.native_buffer.set(planned);
        }
        Ok(())
    }
    fn hint(&self, bytes: usize) -> Result<(), Error> {
        self.check()?;
        self.session.memory(bytes as u128)
    }
    fn value_table(&self, entries: usize) -> Result<(), Error> {
        self.check()?;
        self.session
            .memory(entries as u128 * size_of::<u32>() as u128)?;
        self.session.work(entries as u128)
    }
    fn pixel_checksum(&self, info: &ImageInfo) -> Result<(), Error> {
        self.session.work(crate::pixelsum::hash_work(info)?)
    }

    fn exact(&self, parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        if self.profile == ExecutionProfile::Models {
            return self.check();
        }
        // Decoders hold at most two decoded nested stripes at a time (the one
        // in use and the next one starting in the same unit), plus the
        // exact-block positions of a unit; every exact pixel is decoded once.
        let w = parsed.info.width as u128;
        let h = parsed.info.height as u128;
        let rows = (parsed.hs as u128).min(h);
        self.session
            .memory(2 * w * rows * 3 + 8 * (w / 8) * (h / 8))?;
        self.session.work(w * h * 3)
    }
}

impl<C: Fn() -> bool + Sync> DecodeSession<C> {
    fn units(&self, count: u128) -> Result<(), Error> {
        let mut usage = self.usage.get();
        usage.units = add(usage.units, count, self.policy.units)?;
        self.usage.set(usage);
        self.work(count)
    }
}

// Fixed-size validated state stays on the stack, avoiding an extra allocation.
// Its size does not depend on untrusted image dimensions or record counts.
#[allow(clippy::large_enum_variant)]
enum Prepared<'a> {
    #[cfg(feature = "native-jpeg")]
    Native(crate::jpeg::wire::NativeParsed<'a>),
    Lossless(crate::v1::Parsed<'a>),
    Lossy(crate::lossy::LossyParsed<'a>),
}

fn prepare<'a>(
    data: &'a [u8],
    admission: &impl Admission,
    verify_checksum: bool,
) -> Result<(Prepared<'a>, crate::pixelsum::Table<'a>), Error> {
    let verify = crate::checksum::Verify::from_flag(verify_checksum, 1);
    let f = crate::v1::parse_file(data, admission, verify)?;
    if matches!(f.pixelsum, crate::pixelsum::Table::Known(..)) {
        admission.hint(core::mem::size_of::<crate::pixelsum::Raster>())?;
        admission.pixel_checksum(&f.header.info)?;
    }
    let version = f.header.version;
    let (info, body, declared) = (f.header.info, f.data, f.declared);
    if crate::parse::payload::native_selected(f.header.lossy_color, declared, body)? {
        #[cfg(not(feature = "native-jpeg"))]
        {
            Err(Error::Unsupported("feature native_components"))
        }
        #[cfg(feature = "native-jpeg")]
        {
            let p = crate::jpeg::wire::parse(info, version, body, admission)?;
            if p.payload_len != body.len() {
                return Err(Error::Corrupt("trailing DATA bytes"));
            }
            Ok((Prepared::Native(p), f.pixelsum))
        }
    } else if let Some(color) = f.header.lossy_color {
        let parsed =
            crate::lossy::parse_lossy_checked(info, version, color, declared, body, admission)?;
        if parsed.needs_reference() {
            return Err(Error::Corrupt(
                "keep-reference blocks outside an inter frame",
            ));
        }
        Ok((Prepared::Lossy(parsed), f.pixelsum))
    } else {
        Ok((
            Prepared::Lossless(crate::v1::parse_complete_body_checked(
                info, version, declared, body, admission,
            )?),
            f.pixelsum,
        ))
    }
}

impl Prepared<'_> {
    fn info(&self) -> &ImageInfo {
        match self {
            #[cfg(feature = "native-jpeg")]
            Self::Native(p) => &p.head.info,
            Self::Lossless(p) => &p.info,
            Self::Lossy(p) => &p.info,
        }
    }

    fn decode(
        self,
        out: &mut [u8],
        stride: usize,
        cancel: Option<&(impl Fn() -> bool + Sync)>,
    ) -> Result<ImageInfo, Error> {
        match cancel {
            Some(cancel) => self.decode_checked(out, stride, &|| {
                if cancel() {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            }),
            // Select once per image; the no-cancellation reconstruction has
            // no indirect callback or optional-callback branch at each row.
            None => self.decode_checked(out, stride, &|| Ok(())),
        }
    }

    fn decode_observed(
        self,
        out: &mut [u8],
        stride: usize,
        cancel: Option<&(impl Fn() -> bool + Sync)>,
        pulse: &dyn how_far::Pulse,
    ) -> Result<ImageInfo, Error> {
        let check = || match cancel {
            Some(cancel) if cancel() => Err(Error::Cancelled),
            _ => Ok(()),
        };
        match self {
            Self::Lossless(p) => {
                crate::v1::decode_parsed_into_observed(p, out, stride, &check, pulse)
            }
            Self::Lossy(p) => crate::lossy::decode_lossy_parsed_observed::<true>(
                p,
                out,
                stride,
                1,
                &Limits::NONE,
                true,
                false,
                &check,
                Some(pulse),
            ),
            #[cfg(feature = "native-jpeg")]
            Self::Native(p) => crate::jpeg::wire::decode_observed(p, out, stride, &check, pulse),
        }
    }

    fn decode_checked(
        self,
        out: &mut [u8],
        stride: usize,
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<ImageInfo, Error> {
        match self {
            #[cfg(feature = "native-jpeg")]
            Self::Native(p) => crate::jpeg::wire::decode(p, out, stride, check),
            Self::Lossless(p) => crate::v1::decode_parsed_into_checked(p, out, stride, check),
            Self::Lossy(p) => crate::lossy::decode_lossy_parsed_checked::<true>(
                p,
                out,
                stride,
                1,
                &Limits::NONE,
                true,
                false,
                check,
            ),
        }
    }

    fn decode_rows<F>(
        self,
        row: &mut [u8],
        on_row: F,
        cancel: Option<&(impl Fn() -> bool + Sync)>,
    ) -> Result<ImageInfo, Error>
    where
        F: FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
    {
        match cancel {
            Some(cancel) => self.decode_rows_checked(row, on_row, &|| {
                if cancel() {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            }),
            None => self.decode_rows_checked(row, on_row, &|| Ok(())),
        }
    }

    fn decode_rows_checked<F>(
        self,
        row: &mut [u8],
        on_row: F,
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<ImageInfo, Error>
    where
        F: FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
    {
        let info = self.info().clone();
        check()?;
        match self {
            #[cfg(feature = "native-jpeg")]
            Self::Native(p) => {
                let mut stream = crate::jpeg::wire::NativeStream::new(p)?;
                emit_rows(info, row, on_row, check, |row| {
                    stream.next_row_checked(row, check)
                })
            }
            Self::Lossless(p) => {
                let mut stream = crate::v1::LosslessStream::from_parsed(p);
                emit_rows(info, row, on_row, check, |row| {
                    stream.next_row_checked(row, check)
                })
            }
            Self::Lossy(p) => {
                let mut stream = crate::lossy::LossyStream::from_parsed(p)?;
                emit_rows(info, row, on_row, check, |row| {
                    stream.next_row_checked(row, check)
                })
            }
        }
    }
}

fn emit_rows<F>(
    info: ImageInfo,
    row: &mut [u8],
    mut on_row: F,
    check: &(impl Fn() -> Result<(), Error> + Sync),
    mut next: impl FnMut(&mut [u8]) -> Result<bool, Error>,
) -> Result<ImageInfo, Error>
where
    F: FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>,
{
    let row = &mut row[..info.row_bytes()];
    for y in 0..info.height {
        check()?;
        if !next(row)? {
            return Err(Error::Corrupt("primary image too short"));
        }
        check()?;
        on_row(&info, y, row)?;
    }
    check()?;
    Ok(info)
}

/// Full slice input was already charged by begin(); object/unit work still counts.
pub(crate) struct V4AdmittedInput<'a, C>(&'a DecodeSession<C>);
impl<C> Drop for V4AdmittedInput<'_, C> {
    fn drop(&mut self) {
        debug_assert!(self.0.v4_input_admitted.get() != 0);
        self.0
            .v4_input_admitted
            .set(self.0.v4_input_admitted.get() - 1);
    }
}
