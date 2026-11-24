//! The output image buffer, and the inter-frame reference view.

use crate::error::{Error, Result};

/// A tight row-major, channel-interleaved sample buffer.
#[derive(Clone)]
pub(crate) struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) channels: u8,
    pub(crate) channel_layout: u8,
    pub(crate) sample_bytes: usize,
    pub(crate) buf: Vec<u8>,
}

impl Image {
    pub(crate) fn new(
        width: u32,
        height: u32,
        channels: u8,
        channel_layout: u8,
        sample_bytes: usize,
    ) -> Result<Image> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(channels as usize))
            .and_then(|v| v.checked_mul(sample_bytes))
            .ok_or_else(|| Error::corrupt("image dimensions overflow"))?;
        Ok(Image {
            width,
            height,
            channels,
            channel_layout,
            sample_bytes,
            buf: vec![0; len],
        })
    }

    #[inline]
    fn offset(&self, x: u32, y: u32, channel: usize) -> usize {
        ((y as usize * self.width as usize + x as usize) * self.channels as usize + channel)
            * self.sample_bytes
    }

    pub(crate) fn put_sample(&mut self, x: u32, y: u32, channel: usize, value: u64) {
        let offset = self.offset(x, y, channel);
        let bytes = self.sample_bytes;
        self.buf[offset..offset + bytes].copy_from_slice(&value.to_le_bytes()[..bytes]);
    }

    pub(crate) fn get_sample(&self, x: u32, y: u32, channel: usize) -> u64 {
        let offset = self.offset(x, y, channel);
        let mut word = [0u8; 8];
        word[..self.sample_bytes].copy_from_slice(&self.buf[offset..offset + self.sample_bytes]);
        u64::from_le_bytes(word)
    }

    /// Copy a decoded frame's rows into this canvas at (x0, y0).
    pub(crate) fn blit(&mut self, src: &Image, x0: u32, y0: u32) {
        let row_bytes = src.width as usize * src.channels as usize * src.sample_bytes;
        for row in 0..src.height as usize {
            let src_start = row * row_bytes;
            let dst = ((y0 as usize + row) * self.width as usize + x0 as usize)
                * self.channels as usize
                * self.sample_bytes;
            self.buf[dst..dst + row_bytes]
                .copy_from_slice(&src.buf[src_start..src_start + row_bytes]);
        }
    }
}

/// A read-only view of pixels a frame may predict from (SPEC section 4).
pub(crate) trait Reference {
    fn sample(&self, x: u32, y: u32, channel: usize) -> u64;
}

impl Reference for Image {
    fn sample(&self, x: u32, y: u32, channel: usize) -> u64 {
        self.get_sample(x, y, channel)
    }
}

/// The previous canvas under a frame's rectangle: frame-local coordinates map
/// to canvas coordinates with the frame origin added here.
pub(crate) struct CanvasRef<'a> {
    canvas: &'a Image,
    x0: u32,
    y0: u32,
}

impl<'a> CanvasRef<'a> {
    pub(crate) fn new(canvas: &'a Image, x0: u32, y0: u32) -> Self {
        CanvasRef { canvas, x0, y0 }
    }
}

impl Reference for CanvasRef<'_> {
    fn sample(&self, x: u32, y: u32, channel: usize) -> u64 {
        self.canvas.sample(self.x0 + x, self.y0 + y, channel)
    }
}
