//! Checked offsets into an immutable owned compressed input.
use crate::Error;
use alloc::vec::Vec;
use core::ops::Range;

pub(crate) fn span(source: &[u8], bytes: &[u8]) -> Result<Range<usize>, Error> {
    // Optional records may use the canonical empty slice rather than a slice
    // of the input. No bytes are read in that case.
    if bytes.is_empty() {
        return Ok(0..0);
    }
    let start = (bytes.as_ptr() as usize)
        .checked_sub(source.as_ptr() as usize)
        .ok_or(Error::Corrupt("record outside owned input"))?;
    let end = start.checked_add(bytes.len()).ok_or(Error::TooLarge)?;
    if end > source.len() {
        return Err(Error::Corrupt("record outside owned input"));
    }
    Ok(start..end)
}

pub(crate) fn spans(
    source: &[u8],
    records: &[&[u8]],
    check: &impl Fn() -> Result<(), Error>,
) -> Result<Vec<Range<usize>>, Error> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(records.len())
        .map_err(|_| Error::TooLarge)?;
    for record in records {
        check()?;
        result.push(span(source, record)?);
    }
    Ok(result)
}
