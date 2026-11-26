//! Fallible allocation helpers returning [`Error::TooLarge`] instead of
//! aborting on reservation failure.

use alloc::vec::Vec;

use crate::Error;

/// Fallible zero/value-initialized allocation returning [`Error::TooLarge`] on
/// reservation failure instead of aborting the process.
pub(in crate::lossy) fn try_vec<T: Clone>(len: usize, val: T) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    v.resize(len, val);
    Ok(v)
}

/// Fallible empty `Vec` with exact reserved capacity.
pub(in crate::lossy) fn try_vec_cap<T>(cap: usize) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(cap).map_err(|_| Error::TooLarge)?;
    Ok(v)
}

/// Fallible `Vec::push` returning [`Error::TooLarge`] on allocation failure.
pub(in crate::lossy) fn try_push<T>(v: &mut Vec<T>, val: T) -> Result<(), Error> {
    if v.len() == v.capacity() {
        v.try_reserve(1).map_err(|_| Error::TooLarge)?;
    }
    v.push(val);
    Ok(())
}
