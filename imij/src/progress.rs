//! Decode progress by unit (stripe, column group) for a caller's `Pulse`,
//! lossless and lossy.
//!
//! A decode builds [`UnitProgress`] only for a live pulse (`Pulse::live`). It
//! plans one named, pixel-weighted child per unit, which the worker that
//! decodes the unit starts and finishes. Trackers and profilers then see when
//! each unit ran, and the names (`stripe 3 group 1`) match the units the
//! codestream trace reports. An unobserved decode passes `None` and pays one
//! branch per unit. Unit progress needs the `std` feature (its slots are
//! mutexes); without it `plan` returns `None` and decodes report no units.

use crate::Error;
use alloc::vec::Vec;
#[cfg(feature = "std")]
use alloc::{format, string::String};
use how_far::{Child, Outcome, Pulse};
#[cfg(feature = "std")]
use how_far::{Execution, PhaseSpec, ProgressExt, Total};

#[cfg(feature = "std")]
type Slot<'a> = std::sync::Mutex<Option<Child<'a>>>;
/// Never constructed: without `std`, [`UnitProgress::plan`] returns `None`.
#[cfg(not(feature = "std"))]
type Slot<'a> = core::marker::PhantomData<Child<'a>>;

pub(crate) struct UnitProgress<'a> {
    slots: Vec<Slot<'a>>,
    rows: Vec<u64>,
}

/// Geometry of one unit: stripe, group, first row, rows, first column, columns.
#[cfg_attr(not(feature = "std"), allow(dead_code))]
pub(crate) struct UnitShape {
    pub stripe: usize,
    pub group: usize,
    pub groups: usize,
    pub y: usize,
    pub rows: usize,
    pub x: usize,
    pub cols: usize,
}

impl<'a> UnitProgress<'a> {
    /// Plans one child per unit, or returns `None` when nobody observes the
    /// pulse. `workers` is the decode's concurrency ceiling.
    pub(crate) fn plan(
        pulse: &'a dyn Pulse,
        units: impl ExactSizeIterator<Item = UnitShape>,
        workers: usize,
    ) -> Option<Self> {
        #[cfg(not(feature = "std"))]
        {
            let _ = (pulse, units, workers);
            None
        }
        #[cfg(feature = "std")]
        Self::plan_std(pulse, units, workers)
    }

    /// Plans one child per MCU row band of a native JPEG payload (SPEC
    /// 2.12, one unit record each), named by band and pixel rows: `band 3
    /// (rows 48-63)`. `bands` yields each band's first row and row count.
    /// One worker decodes them in order.
    #[cfg(feature = "native-jpeg")]
    pub(crate) fn plan_bands(
        pulse: &'a dyn Pulse,
        bands: impl ExactSizeIterator<Item = (usize, usize)>,
        cols: usize,
    ) -> Option<Self> {
        #[cfg(not(feature = "std"))]
        {
            let _ = (pulse, bands, cols);
            None
        }
        #[cfg(feature = "std")]
        {
            pulse.live()?;
            let n = bands.len();
            let mut names: Vec<String> = Vec::with_capacity(n);
            let mut weights = Vec::with_capacity(n);
            let mut rows = Vec::with_capacity(n);
            for (b, (y, r)) in bands.enumerate() {
                names.push(format!("band {b} (rows {}-{})", y, y + r - 1));
                weights.push(((r * cols) as u64).max(1));
                rows.push(r as u64);
            }
            Self::plan_parts(pulse, names, weights, rows, 1)
        }
    }

    #[cfg(feature = "std")]
    fn plan_std(
        pulse: &'a dyn Pulse,
        units: impl ExactSizeIterator<Item = UnitShape>,
        workers: usize,
    ) -> Option<Self> {
        pulse.live()?;
        let n = units.len();
        let mut names: Vec<String> = Vec::with_capacity(n);
        let mut weights = Vec::with_capacity(n);
        let mut rows = Vec::with_capacity(n);
        for u in units {
            names.push(if u.groups > 1 {
                format!(
                    "stripe {} group {} (rows {}-{}, columns {}-{})",
                    u.stripe,
                    u.group,
                    u.y,
                    u.y + u.rows - 1,
                    u.x,
                    u.x + u.cols - 1
                )
            } else {
                format!("stripe {} (rows {}-{})", u.stripe, u.y, u.y + u.rows - 1)
            });
            weights.push(((u.rows * u.cols) as u64).max(1));
            rows.push(u.rows as u64);
        }
        Self::plan_parts(pulse, names, weights, rows, workers)
    }

    /// One child per named, weighted part of `rows` rows each.
    #[cfg(feature = "std")]
    fn plan_parts(
        pulse: &'a dyn Pulse,
        names: Vec<String>,
        weights: Vec<u64>,
        rows: Vec<u64>,
        workers: usize,
    ) -> Option<Self> {
        let pulse = pulse.live()?;
        if names.is_empty() {
            return None;
        }
        let parts: Vec<PhaseSpec<'_>> = names
            .iter()
            .zip(&weights)
            .zip(&rows)
            .map(|((name, &w), &r)| PhaseSpec::new(name, w, Total::Exact(r)).units("rows"))
            .collect();
        let pool =
            core::num::NonZeroUsize::new(workers.max(1)).unwrap_or(core::num::NonZeroUsize::MIN);
        let children = pulse.plan(Execution::work_pool(pool), &parts);
        Some(Self {
            slots: children
                .into_iter()
                .map(|c| std::sync::Mutex::new(Some(c)))
                .collect(),
            rows,
        })
    }

    #[cfg(feature = "std")]
    fn take(&self, u: usize) -> Option<Child<'a>> {
        self.slots.get(u)?.lock().ok()?.take()
    }

    #[cfg(not(feature = "std"))]
    fn take(&self, _: usize) -> Option<Child<'a>> {
        None
    }

    #[cfg(feature = "std")]
    fn put(&self, u: usize, child: Child<'a>) {
        if let Some(Ok(mut slot)) = self.slots.get(u).map(|s| s.lock()) {
            *slot = Some(child);
        }
    }

    #[cfg(not(feature = "std"))]
    fn put(&self, _: usize, _: Child<'a>) {}

    /// The worker that decodes unit `u` is starting it. The checkpoint after
    /// `start` is where profilers (how-far-really) open the unit's span;
    /// without it the span would begin at the final report. It is also the
    /// unit's first cancellation check: a stop seen here fails the unit, since
    /// a stop that reports once would otherwise be consumed and lost. Callers
    /// pass the result to [`Self::end`], which records it as cancelled.
    pub(crate) fn begin(&self, u: usize) -> Result<(), Error> {
        if let Some(child) = self.take(u) {
            let pulse = &child as &dyn Pulse;
            let _ = pulse.start();
            let checked = pulse.check();
            self.put(u, child);
            checked.map_err(|_| Error::Cancelled)?;
        }
        Ok(())
    }

    /// Unit `u` finished, with the decode's result for it.
    pub(crate) fn end(&self, u: usize, result: &Result<(), Error>) {
        if let Some(child) = self.take(u) {
            let outcome = match result {
                Ok(()) => {
                    (&child as &dyn Pulse).advance(self.rows[u]);
                    Outcome::Succeeded
                }
                Err(Error::Cancelled) => Outcome::Cancelled,
                Err(_) => Outcome::Failed,
            };
            let _ = child.finish(outcome);
        }
    }
}

/// Units that never ran (an earlier unit failed, or the decode stopped) are
/// reported as not run rather than abandoned.
impl Drop for UnitProgress<'_> {
    fn drop(&mut self) {
        for u in 0..self.slots.len() {
            if let Some(child) = self.take(u) {
                child.complete_inferred(Outcome::NotRun);
            }
        }
    }
}
