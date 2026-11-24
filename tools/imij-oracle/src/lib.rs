//! An independent IMI decoder, written from `SPEC.md` alone (format versions 2 and 3).
//!
//! This crate shares no code with the reference codec. It decodes the lossless
//! paths (container, header, units, entropy models, rANS, reconstruction,
//! animation and pages) and reports the specification's two error classes:
//! [`Error::Unsupported`] for syntax a decoder does not implement and
//! [`Error::Corrupt`] for invalid streams.
//!
//! The decoder is deliberately scalar and simple: it exists to be a second
//! implementation of the specification, not a fast one.

#![forbid(unsafe_code)]

mod bits;
pub mod check;
mod container;
mod error;
mod frames;
mod header;
mod image;
mod maps;
mod model;
mod payload;
mod quick;
mod rans;
mod registry;
mod sha256;
mod stripe;
mod template;

pub use error::Error;
pub use header::{Header, SampleType};
pub use sha256::Sha256;

/// The decoded output of one complete IMI file.
#[derive(Debug)]
pub struct Decoded {
    /// Width of the primary image (the canvas for animations).
    pub width: u32,
    /// Output height of the primary image.
    pub height: u32,
    /// Channel count of the primary image (1..=4).
    pub channels: u8,
    /// The primary image's sample type code (0..=10).
    pub sample: u8,
    /// One raw pixel buffer per frame or page, in decode order. A still image
    /// has exactly one. The layout is the parity tools' raw format:
    /// row-major, channel-interleaved, little-endian samples.
    pub frames: Vec<Vec<u8>>,
}

/// Decode one complete IMI file, with no external reference frame.
pub fn decode(data: &[u8]) -> Result<Decoded, Error> {
    let (images, header) = frames::decode_file(data, None)?;
    Ok(Decoded {
        width: header.width,
        height: header.height,
        channels: header.channels,
        sample: header.sample_code,
        frames: images.into_iter().map(|img| img.buf).collect(),
    })
}
