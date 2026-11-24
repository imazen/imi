//! Minimal scoped parallel map over independent work items.

use alloc::vec::Vec;

/// The built-in scheduler can spawn native threads, but std's presence alone
/// does not imply a thread host (notably on WASI). On browser WASM a `rayon`
/// build runs the same units on wasm-bindgen-rayon's Web Worker pool: the
/// caller (the browser wrapper crate) always initializes that pool before any
/// decode, and `rayon::scope` then uses it. WASI builds stay serial.
#[inline]
pub(crate) fn worker_count(requested: usize, items: usize) -> usize {
    if cfg!(all(feature = "std", not(target_family = "wasm")))
        || cfg!(all(
            feature = "std",
            feature = "rayon",
            target_arch = "wasm32",
            target_os = "unknown"
        ))
    {
        requested.max(1).min(items)
    } else {
        items.min(1)
    }
}

/// Accumulates independent items into at most one value per worker.
/// Round-robin assignment spreads spatially uneven image work between workers.
/// Unlike `par_map`, accumulator storage does not grow with the item count.
#[cfg(feature = "encode")]
pub(crate) fn par_fold<I: Sync, T: Send>(
    items: &[I],
    threads: usize,
    init: impl Fn() -> T + Sync,
    fold: impl Fn(&mut T, &I) + Sync,
) -> Vec<T> {
    let workers = worker_count(threads, items.len());
    par_map((0..workers).collect(), threads, |_, worker| {
        let mut acc = init();
        for item in items[worker..].iter().step_by(workers) {
            fold(&mut acc, item);
        }
        acc
    })
}

/// Runs `f` once per worker on up to `threads` workers and returns when all
/// have finished. Native std builds use scoped threads — the lossless decode
/// driver's historical spawning; a browser WASM build cannot spawn native
/// threads, so there the same calls run on the rayon Web Worker pool the
/// caller initialized (wasm-bindgen-rayon). A `rayon` build for browser WASM
/// must initialize that pool before any decode.
#[cfg(feature = "std")]
pub(crate) fn scoped<F: Fn() + Send + Sync>(threads: usize, f: F) {
    #[cfg(all(feature = "rayon", target_arch = "wasm32", target_os = "unknown"))]
    rayon::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|_| f());
        }
    });
    #[cfg(not(all(feature = "rayon", target_arch = "wasm32", target_os = "unknown")))]
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(&f);
        }
    });
}

/// Quick units are inexpensive enough that repeatedly creating OS threads can
/// dominate their decode. Reuse the caller's Rayon pool when available; keep
/// the existing scoped-thread fallback for builds without a pool host.
#[cfg(feature = "std")]
pub(crate) fn scoped_reuse<F: Fn() + Send + Sync>(threads: usize, reuse: bool, f: F) {
    #[cfg(feature = "rayon")]
    if reuse {
        rayon::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|_| f());
            }
        });
        return;
    }
    let _ = reuse;
    scoped(threads, f);
}

/// Consume disjoint, already partitioned work without per-item ownership locks.
/// The caller bounds the number of bands by the requested workers.
#[cfg(feature = "std")]
pub(crate) fn bands<I: Send>(items: Vec<I>, f: impl Fn(I) + Send + Sync) {
    if items.len() <= 1 {
        for item in items {
            f(item);
        }
        return;
    }
    #[cfg(feature = "rayon")]
    rayon::scope(|scope| {
        for item in items {
            let f = &f;
            scope.spawn(move |_| f(item));
        }
    });
    #[cfg(not(feature = "rayon"))]
    std::thread::scope(|scope| {
        for item in items {
            let f = &f;
            scope.spawn(move || f(item));
        }
    });
}

/// Runs `f(i, item)` for every item, using up to `threads` threads (std;
/// with the `rayon` feature, tasks on the current rayon pool) or
/// sequentially (no_std / WASM / threads <= 1). Results keep item order.
pub(crate) fn par_map<I: Send, T: Send, F>(items: Vec<I>, threads: usize, f: F) -> Vec<T>
where
    F: Fn(usize, I) -> T + Sync,
{
    let threads = worker_count(threads, items.len());
    #[cfg(feature = "std")]
    if threads > 1 && items.len() > 1 {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;
        let n = items.len();
        let slots: Vec<Mutex<Option<I>>> = items.into_iter().map(|i| Mutex::new(Some(i))).collect();
        let results: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        // One worker body and ownership scheme; only spawning is platform-
        // specific. Concrete closures keep the adapter statically dispatched.
        let worker = || loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                break;
            }
            let item = slots[i].lock().unwrap().take().unwrap();
            let r = f(i, item);
            *results[i].lock().unwrap() = Some(r);
        };
        #[cfg(feature = "rayon")]
        rayon::scope(|sc| {
            for _ in 0..threads.min(n) {
                sc.spawn(|_| worker());
            }
        });
        #[cfg(not(feature = "rayon"))]
        std::thread::scope(|sc| {
            for _ in 0..threads.min(n) {
                sc.spawn(worker);
            }
        });
        return results
            .into_iter()
            .map(|m| m.into_inner().unwrap().unwrap())
            .collect();
    }
    let _ = threads;
    items
        .into_iter()
        .enumerate()
        .map(|(i, it)| f(i, it))
        .collect()
}
