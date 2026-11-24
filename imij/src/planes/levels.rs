//! Direct-from-original level generation without a codec-specific resizer.
use super::{LevelDescriptor, LevelSource, PlaneId, PlaneSetBuilder, extent, sample_bytes};
use crate::{Error, SampleType};
use alloc::vec::Vec;

/// Borrowed original scalar plane. Every requested level can reuse this same
/// reference, avoiding a chain of downscales from previously decoded levels.
///
/// This API deliberately leaves filtering, transfer functions, alpha treatment
/// and numerical-plane semantics to the caller's resizer. For RGB, resize the
/// original color image with the desired color/alpha policy before splitting
/// it into planes; independently resizing encoded color channels need not be
/// equivalent. Depth, masks and IDs may require different reduction policies.
#[derive(Clone, Copy, Debug)]
pub struct OriginalPlane<'a> {
    pixels: &'a [u8],
    width: u32,
    height: u32,
    stride: usize,
    sample: SampleType,
}
impl<'a> OriginalPlane<'a> {
    pub fn new(
        pixels: &'a [u8],
        width: u32,
        height: u32,
        stride_bytes: usize,
        sample: SampleType,
    ) -> Result<Self, Error> {
        let len = extent(width, height, sample, stride_bytes)?;
        if pixels.len() < len {
            return Err(Error::InvalidInput("original plane extent"));
        }
        Ok(Self {
            pixels: &pixels[..len],
            width,
            height,
            stride: stride_bytes,
            sample,
        })
    }
    pub fn pixels(self) -> &'a [u8] {
        self.pixels
    }
    pub fn width(self) -> u32 {
        self.width
    }
    pub fn height(self) -> u32 {
        self.height
    }
    pub fn stride_bytes(self) -> usize {
        self.stride
    }
    pub fn sample_type(self) -> SampleType {
        self.sample
    }
}

impl PlaneSetBuilder {
    /// Generates one level directly from `original`, then independently codes
    /// it losslessly or lossily. The callback receives the original every time;
    /// no previous encoded or reconstructed level is supplied to it.
    ///
    /// `resize` writes a tight target plane of the same sample type. A lossless
    /// level reproduces these reference bytes exactly. Lossy coding requires
    /// separate perceptual/error validation against that reference; neither
    /// this declaration nor a quality setting guarantees indistinguishability.
    /// Callback errors leave the builder unchanged. Resizer scratch is owned
    /// and bounded by the caller; the target allocation obeys the builder's
    /// decoded-work limit. This is a generic callback, not a dynamic interface.
    pub fn add_level_from_original(
        &mut self,
        plane: PlaneId,
        original: &OriginalPlane<'_>,
        level: LevelDescriptor,
        coding: crate::ImiEncoderConfig,
        resize: impl FnOnce(&OriginalPlane<'_>, u32, u32, &mut [u8], usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.validate_level_request(plane, &level, &coding)?;
        let descriptor = self
            .planes()
            .get(plane.index() as usize)
            .ok_or(Error::InvalidInput("unknown plane"))?;
        if descriptor.sample_type() != original.sample {
            return Err(Error::InvalidInput("original plane sample type"));
        }
        level.tiles()?;
        let row = sample_bytes(level.width(), 1, original.sample)?;
        let len = sample_bytes(level.width(), level.height(), original.sample)?;
        if len as u64 > self.limits().max_decoded_work {
            return Err(Error::TooLarge);
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
        pixels.resize(len, 0);
        resize(original, level.width(), level.height(), &mut pixels, row)?;
        self.add_level(
            plane,
            level.with_source(LevelSource::DirectFromOriginal),
            &pixels,
            row,
            coding,
        )
        .map(|_| ())
    }
}
