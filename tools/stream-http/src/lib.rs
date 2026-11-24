//! End-to-end streaming encode over HTTP.
//!
//! One codec table ([`Codec`]), one source image ([`Source`]) and one encode
//! loop ([`stream_encode`] for zencodec's `begin`/`pending_output`,
//! [`one_shot`] for `encode`), shared by the Rust server (`bin/server.rs`),
//! the load client (`bin/client.rs`) and the C ABI the ASP.NET server loads
//! (`ffi.rs`). Every encoder is reached through `DynEncoder`, as a
//! codec-agnostic server would.

#![deny(unsafe_code)]

pub mod ffi;

use zencodec::encode::{DynEncoder, EncodeJob, EncoderConfig, OutputTiming};
use zenpixels::{PixelDescriptor, PixelSlice};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// RGB8 sRGB, the only layout the bench feeds.
pub const RGB8: PixelDescriptor = PixelDescriptor::RGB8_SRGB;

/// Working-storage allowance for the IMIJ row encoders.
const IMIJ_WORKING: usize = 64 << 20;

/// The encoders under test, by the name the HTTP query uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// IMIJ quick pixel packets (QOI-style) in row groups.
    ImijQuick,
    /// IMIJ MED prediction with fixed rANS tables, in row groups.
    ImijGroup,
    /// zenjpeg q85 4:2:0, fixed Huffman tables, sequential: streams.
    JpegFixed,
    /// zenjpeg q85 4:2:0 default (progressive, optimized tables): at finish.
    JpegDefault,
    /// zenpng effort 0 (stored), downcasts off: streams row by row.
    PngE0,
    /// zenpng effort 1, downcasts off, sequential: streams ~512 KiB strips.
    PngE1,
    /// zenpng effort 5, downcasts off, sequential: streams ~512 KiB strips.
    PngE5,
    /// zenbitmaps binary PPM (P6): streams row by row.
    Ppm,
    /// zenbitmaps QOI: streams row by row.
    Qoi,
}

impl Codec {
    pub const ALL: [Codec; 9] = [
        Codec::ImijQuick,
        Codec::ImijGroup,
        Codec::JpegFixed,
        Codec::JpegDefault,
        Codec::PngE0,
        Codec::PngE1,
        Codec::PngE5,
        Codec::Ppm,
        Codec::Qoi,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Codec::ImijQuick => "imij-quick",
            Codec::ImijGroup => "imij-group",
            Codec::JpegFixed => "jpeg-fixed",
            Codec::JpegDefault => "jpeg-default",
            Codec::PngE0 => "png-e0",
            Codec::PngE1 => "png-e1",
            Codec::PngE5 => "png-e5",
            Codec::Ppm => "ppm",
            Codec::Qoi => "qoi",
        }
    }

    pub fn parse(name: &str) -> Option<Codec> {
        Codec::ALL.into_iter().find(|c| c.name() == name)
    }

    pub fn mime(self) -> &'static str {
        match self {
            Codec::ImijQuick | Codec::ImijGroup => "image/x-imij",
            Codec::JpegFixed | Codec::JpegDefault => "image/jpeg",
            Codec::PngE0 | Codec::PngE1 | Codec::PngE5 => "image/png",
            Codec::Ppm => "image/x-portable-pixmap",
            Codec::Qoi => "image/qoi",
        }
    }

    /// A fresh encoder for a `width`-wide image.
    pub fn encoder(self, width: u32) -> Result<Box<dyn DynEncoder>, Error> {
        use zenpng::{Compression, DowncastFlags, PngEncoderConfig};
        let png = |effort| {
            PngEncoderConfig::new()
                .with_compression(Compression::Effort(effort))
                .with_downcast(DowncastFlags::none())
                .with_threading(zencodec::ThreadingPolicy::Sequential)
        };
        let jpeg =
            zenjpeg::JpegEncoderConfig::ycbcr(85.0, zenjpeg::encoder::ChromaSubsampling::Quarter);
        Ok(match self {
            Codec::ImijQuick => imij::ImiEncoderConfig::quick()
                .with_row_groups(Some(imij::RowGroups::new(width, 1, IMIJ_WORKING)))
                .job()
                .dyn_encoder()?,
            Codec::ImijGroup => imij::ImiEncoderConfig::default()
                .with_row_groups(Some(imij::RowGroups::new(width, 1, IMIJ_WORKING)))
                .job()
                .dyn_encoder()?,
            Codec::JpegFixed => jpeg
                .with_progressive(false)
                .with_huffman(false)
                .job()
                .dyn_encoder()?,
            Codec::JpegDefault => jpeg.job().dyn_encoder()?,
            Codec::PngE0 => png(0).job().dyn_encoder()?,
            Codec::PngE1 => png(1).job().dyn_encoder()?,
            Codec::PngE5 => png(5).job().dyn_encoder()?,
            Codec::Ppm => zenbitmaps::PnmEncoderConfig::new().job().dyn_encoder()?,
            Codec::Qoi => zenbitmaps::QoiEncoderConfig::new().job().dyn_encoder()?,
        })
    }
}

/// An RGB8 image the bench crops requests from (rows stay strided).
pub struct Source {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Source {
    /// Decode a JPEG or PNG photo to RGB8.
    pub fn load(path: &std::path::Path) -> Result<Source, Error> {
        use zencodec::decode::{Decode, DecodeJob, DecoderConfig};
        let bytes = std::fs::read(path)?;
        let want = [RGB8];
        let out = if bytes.starts_with(&[0xFF, 0xD8]) {
            zenjpeg::JpegDecoderConfig::new()
                .job()
                .decoder(std::borrow::Cow::Borrowed(&bytes), &want)?
                .decode()?
        } else {
            zenpng::PngDecoderConfig::new()
                .job()
                .decoder(std::borrow::Cow::Borrowed(&bytes), &want)?
                .decode()?
        };
        let px = out.pixels();
        if px.descriptor().layout() != RGB8.layout()
            || px.descriptor().channel_type() != RGB8.channel_type()
        {
            return Err(format!("decoded {:?}, want RGB8", px.descriptor()).into());
        }
        Ok(Source {
            width: px.width(),
            height: px.rows(),
            pixels: px.contiguous_bytes().into_owned(),
        })
    }

    /// Incompressible RGB8 noise (xorshift), for encoders' raw fallbacks.
    pub fn noise(width: u32, height: u32) -> Source {
        let mut x = 0x2545_f491u32;
        let pixels = (0..width as usize * height as usize * 3)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect();
        Source {
            width,
            height,
            pixels,
        }
    }

    pub fn stride(&self) -> usize {
        self.width as usize * 3
    }

    /// Rows `y..y + rows` of the top-left `width`-wide crop.
    pub fn rows(&self, width: u32, y: u32, rows: u32) -> PixelSlice<'_> {
        let stride = self.stride();
        let start = y as usize * stride;
        let end = start + (rows as usize - 1) * stride + width as usize * 3;
        PixelSlice::new(&self.pixels[start..end], width, rows, stride, RGB8)
            .expect("crop within the source")
    }

    /// Clamp a requested crop to the source.
    pub fn crop(&self, width: u32, height: u32) -> (u32, u32) {
        (width.clamp(1, self.width), height.clamp(1, self.height))
    }
}

/// Encode the `width` x `height` crop through `begin`/`push_rows`/
/// `pending_output`, `strip` rows per push, handing every pending byte to
/// `sink` as soon as it exists. Returns `begin`'s timing and the bytes sent.
pub fn stream_encode(
    codec: Codec,
    src: &Source,
    (width, height): (u32, u32),
    strip: u32,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), Error>,
) -> Result<(OutputTiming, u64), Error> {
    let mut enc = codec.encoder(width)?;
    let timing = enc.begin(width, height, RGB8)?;
    let mut sent = 0u64;
    let mut drain = |enc: &mut Box<dyn DynEncoder>| -> Result<(), Error> {
        let pending = enc.pending_output();
        if !pending.is_empty() {
            let n = pending.len();
            sink(pending)?;
            enc.consume_output(n);
            sent += n as u64;
        }
        Ok(())
    };
    drain(&mut enc)?;
    let mut y = 0;
    while y < height {
        let rows = strip.min(height - y);
        enc.push_rows(src.rows(width, y, rows))?;
        drain(&mut enc)?;
        y += rows;
    }
    let tail = enc.finish()?;
    sink(tail.data())?;
    Ok((timing, sent + tail.data().len() as u64))
}

/// One-shot `encode` of the `width` x `height` crop.
pub fn one_shot(codec: Codec, src: &Source, (width, height): (u32, u32)) -> Result<Vec<u8>, Error> {
    let enc = codec.encoder(width)?;
    Ok(enc.encode(src.rows(width, 0, height))?.into_vec())
}
