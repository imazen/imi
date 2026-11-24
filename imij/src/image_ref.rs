//! Borrowed native samples shared by encoder and decoder entry points.

use crate::ImageInfo;

/// Raw samples with an IMI [`ImageInfo`]: rows of `info.width` pixels,
/// `stride` bytes apart, never copied.
///
/// The ordinary input is a [`zenpixels::PixelSlice`]; this view is for what zenpixels
/// cannot describe yet (32- and 64-bit integers, signed integers, `F64`,
/// significant-bit declarations). V4 group decoding also uses it for decoded
/// dependencies. The view is available in decoder-only builds.
#[derive(Clone, Copy, Debug)]
pub struct ImageRef<'a> {
    pub pixels: &'a [u8],
    pub stride: usize,
    pub info: &'a ImageInfo,
}

impl<'a> ImageRef<'a> {
    /// A view with an explicit row stride in bytes.
    pub const fn new(pixels: &'a [u8], stride: usize, info: &'a ImageInfo) -> Self {
        Self {
            pixels,
            stride,
            info,
        }
    }

    /// A view of tightly packed rows (`stride` is one row's bytes).
    pub fn packed(pixels: &'a [u8], info: &'a ImageInfo) -> Self {
        Self::new(pixels, info.row_bytes(), info)
    }
}
