//! Browser-owned allocations obey the decoder's default output limits before
//! allocation, including padded canvases and the concatenated frame result.
use imij::Error;
#[cfg(any(feature = "full", test))]
use imij::{ImageInfo, Limits};

#[cfg(any(feature = "full", test))]
pub(super) fn layout(info: &ImageInfo, padding: usize) -> Result<(usize, usize), Error> {
    let pixels = u64::from(info.width) * u64::from(info.height);
    let row = (info.width as usize)
        .checked_mul(info.bytes_per_pixel())
        .ok_or(Error::TooLarge)?;
    let stride = row.checked_add(padding).ok_or(Error::TooLarge)?;
    let size = stride
        .checked_mul(info.height as usize)
        .ok_or(Error::TooLarge)?;
    if pixels > Limits::DEFAULT.max_pixels || size as u64 > Limits::DEFAULT.max_bytes {
        return Err(Error::TooLarge);
    }
    Ok((stride, size))
}

pub(super) fn filled(size: usize, value: u8) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
    out.resize(size, value);
    Ok(out)
}

#[cfg(any(feature = "full", test))]
pub(super) fn reserve_result(out: &mut Vec<u8>, additional: usize) -> Result<(), Error> {
    let size = out.len().checked_add(additional).ok_or(Error::TooLarge)?;
    if size as u64 > Limits::DEFAULT.max_bytes {
        return Err(Error::TooLarge);
    }
    out.try_reserve(additional).map_err(|_| Error::TooLarge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imij::{Channels, SampleType};

    #[test]
    fn bounds_precede_allocation_and_include_padding() {
        let image = ImageInfo::new(17, 3, Channels::Rgb, SampleType::U16);
        assert_eq!(layout(&image, 13), Ok((115, 345)));
        assert_eq!(layout(&image, usize::MAX), Err(Error::TooLarge));
        let large = ImageInfo::new(120_000_001, 1, Channels::Gray, SampleType::U8);
        assert_eq!(layout(&large, 0), Err(Error::TooLarge));
        let bytes = ImageInfo::new(120_000_000, 1, Channels::Rgba, SampleType::U64);
        assert_eq!(layout(&bytes, 0), Err(Error::TooLarge));
        let mut result = vec![1, 2, 3];
        assert_eq!(
            reserve_result(&mut result, Limits::DEFAULT.max_bytes as usize),
            Err(Error::TooLarge)
        );
        assert_eq!(result, [1, 2, 3]);
        assert_eq!(
            reserve_result(&mut result, usize::MAX),
            Err(Error::TooLarge)
        );
    }
}
