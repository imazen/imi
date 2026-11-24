//! Exact shared timing and conservative millisecond resource admission.
use crate::{Error, FrameDuration};

pub(super) fn common(duration: FrameDuration) -> Result<zencodec::animation::FrameDuration, Error> {
    zencodec::animation::FrameDuration::new(duration.num, duration.den)
        .map_err(|_| Error::Corrupt("bad frame duration"))
}

// Round each frame upwards for admission only. u128 holds every native u64
// numerator in milliseconds, and their sum over the u32 frame-count domain.
pub(super) fn admission_ms(duration: FrameDuration) -> Result<u128, Error> {
    common(duration)?;
    Ok((u128::from(duration.num) * 1000).div_ceil(u128::from(duration.den)))
}

pub(super) fn check_limit(ms: u128, limits: &zencodec::ResourceLimits) -> Result<(), Error> {
    if limits
        .max_animation_ms
        .is_some_and(|max| ms > u128::from(max))
    {
        return Err(Error::TooLarge);
    }
    Ok(())
}
