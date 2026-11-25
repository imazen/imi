//! imij — experimental fast lossless image format.
//!
//! Design goals: QOI-class decode speed (>1 GB/s of output on one core),
//! better compression than QOI on photos and screen content, 8-bit, 16-bit
//! and f16/f32 samples (signed integers and 32-/64-bit decoding with the
//! `extended` feature), CICP color signaling and HDR metadata. See `SPEC.md`.
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
// Lane-indexed loops mirror the SIMD kernels they reference; kernel entry
// points take many slices by design.
#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

extern crate alloc;

/// Shared alpha interpretation, including explicit opacity and undefined padding.
pub use zenpixels::AlphaMode;

mod adapt_color;
mod bilevel;
mod bits;
mod checksum;
/// Reusable zencodec configuration and per-operation jobs.
pub mod codec;
mod coding_layout;
/// Review-only native component interpretation, typed group encoding and
/// forward decoding with shared planar types and final integrity validation.
#[cfg(feature = "lab")]
pub mod components;
#[cfg(feature = "encode")]
mod config;
pub mod hash;
#[cfg(feature = "lab")]
pub use config::LossyLab;
#[cfg(feature = "encode")]
pub use config::{Coder, ImiEncoderConfig, LosslessTuning, LossyTuning, RowGroups};
pub use hash::{Digest, HashAlgorithm, Hasher};
mod image_ref;
pub use image_ref::ImageRef;
mod color_origin;
mod container;
pub mod decode_resources;
mod decoder_memory;
mod effort;
#[cfg(feature = "encode")]
mod encode;
mod float_guarantees;
mod float_quant;
pub use float_guarantees::FloatGuarantees;
mod frames;
/// Native JPEG quantized components and defined reconstruction.
#[cfg(feature = "native-jpeg")]
pub mod jpeg;
mod lossy;
pub mod mapping;
mod mathx;
mod metadata;
mod metadata_pack;
mod near;
mod owned;
mod par;
mod parse;
mod pixelsum;
/// Typed planes, independent resolution levels and seekable region access.
pub mod planes;
mod predict;
mod progress;
mod quick;
mod template;
#[cfg(feature = "std")]
mod traversal;
mod upgrade;
#[cfg(feature = "std")]
pub mod validation;
#[cfg(feature = "encode")]
pub(crate) use lossy::LossyConfig;
#[cfg(all(test, feature = "encode"))]
pub(crate) use lossy::encode_lossy;
pub use quick::QuickBitSmarts;
#[cfg(feature = "encode")]
pub(crate) use v1::EncodeConfig;
#[cfg(all(test, feature = "encode"))]
pub(crate) use v1::encode;
mod px;
mod rans;
#[cfg(target_arch = "aarch64")]
mod rans_neon;
#[cfg(any(
    target_arch = "aarch64",
    all(target_arch = "wasm32", target_feature = "simd128"),
    all(test, feature = "encode")
))]
mod rans_portable;
mod registry;
// The wasm128 reconstruction tiers dispatch from `v1`; `test` compiles the
// module everywhere so its scalar tier is checked against the reference.
pub mod item_metadata;
#[cfg(feature = "std")]
mod read;
mod recon;
#[cfg(any(
    target_arch = "aarch64",
    all(target_arch = "wasm32", target_feature = "simd128"),
    all(test, feature = "encode")
))]
#[allow(dead_code)]
mod recon_portable;
/// Region decoding of the primary image through its unit table hint (SPEC 2.7)
/// and the DATA locator.
pub mod region;
/// Calibration, missing data, units, axes, coordinates, categories and
/// structured metadata records (SPEC.md 1.5). Ignorable: they never change
/// pixels.
pub mod science;
#[cfg(all(test, feature = "std", feature = "encode"))]
mod test_sha256;
#[allow(dead_code)]
mod wasm_relaxed;
// Seed tables: crate-private; the lab build exposes its training hooks.
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod seeds;
#[cfg(not(feature = "lab"))]
mod seeds;
#[cfg(feature = "encode")]
pub mod stream_encode;
mod tokens;
mod trace;
mod v1;
/// Version 4 wire components. Production reader/writer integration is in progress.
pub mod v4;
mod wide;

#[cfg(feature = "lab")]
mod ans;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod bilevel_lab;
#[cfg(feature = "lab")]
mod huffman;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod kbit_lab;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod lab;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod lab_huff;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod lab_rans;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub mod lab_rice;

use alloc::vec::Vec;

pub use container::{Chunk, GainMap};

pub use float_quant::FloatQuantization;
#[cfg(feature = "encode")]
pub use frames::{
    FrameConfig, FrameEncoder, ImageReference, SequenceEncoder, SequenceItem, SequenceKind,
};
pub use frames::{
    FrameDecoder, FrameDuration, FrameHeader, FrameKind, FrameSeekInfo, IndexedAttribute,
    IndexedMetadataQuery, IndexedMetadataSummary, IndexedReference, check_indexed_frame_prefix,
    decode_indexed_nested, decode_indexed_nested_with_metadata, probe_indexed_nested,
    probe_indexed_nested_summary,
};
pub use lossy::ColorTransform;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::LossyAqField;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::LossyLabSwitches;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::LossyTrellis;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::ReconBias;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::SmallLab;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::chroma_lab;
#[cfg(feature = "lab")]
#[doc(hidden)]
pub use lossy::lab_base_tables;
pub use lossy::{
    ButteraugliNorm, ChannelSampling, LossyChannelLayout, LossyColor, LossyEntropy, LossyQuality,
    LossyTransform,
};
pub use metadata::compress_metadata;
pub use parse::header::{
    Coding, DecodeSpeed, Feature, Features, Header, Profile, ProfileVersion, supported_features,
    supported_profiles,
};
pub use pixelsum::{PixelChecksumStatus, add_pixel_checksum, add_pixel_checksum_with};
#[cfg(feature = "std")]
pub use read::decode_reader;
pub use upgrade::{UpgradeLimits, upgrade_image};
#[doc(hidden)]
pub use v1::simd_tier;
pub use v1::{StreamDecoder, decode, decode_into, decode_with_limits, header, previews, probe};
#[cfg(feature = "std")]
pub use v1::{decode_into_threads, decode_into_threads_with_limits};
/// Gain map parameter types (ISO 21496-1), from zencodec.
pub use zencodec::{GainMapChannel, GainMapParams};

#[cfg(feature = "lab")]
pub use encode::{Analysis, AnalysisConfig, analyze, default_lanes};
#[cfg(feature = "lab")]
pub use lab_huff::{SizeReport, encode_huffman, encode_report};
#[cfg(feature = "lab")]
pub use v1::lane_utilization;

/// Channel layout (header `channels`, SPEC 1.0). Allowed wire values depend
/// on the header version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum Channels {
    Gray = 1,
    GrayAlpha = 2,
    Rgb = 3,
    Rgba = 4,
    /// Native C, M, Y, K ink components; K is not alpha.
    Cmyk = 5,
    /// Native Oklab L, a, b components, without RGB conversion.
    Oklab = 6,
    /// Native Oklab L, a, b components followed by alpha.
    OklabAlpha = 7,
    /// One non-color component in numeric coding (header versions 3 and 4).
    Scalar = 8,
}

impl Channels {
    pub fn count(self) -> usize {
        match self {
            Self::Gray | Self::Scalar => 1,
            Self::GrayAlpha => 2,
            Self::Rgb | Self::Oklab => 3,
            Self::Rgba | Self::Cmyk | Self::OklabAlpha => 4,
        }
    }
    pub fn has_alpha(self) -> bool {
        matches!(
            self,
            Channels::GrayAlpha | Channels::Rgba | Channels::OklabAlpha
        )
    }
    pub fn color_count(self) -> usize {
        match self {
            Channels::Gray | Channels::GrayAlpha => 1,
            Channels::Rgb | Channels::Rgba | Channels::Oklab | Channels::OklabAlpha => 3,
            Channels::Cmyk => 4,
            Channels::Scalar => 0,
        }
    }
    pub(crate) fn is_native_model(self) -> bool {
        matches!(
            self,
            Self::Cmyk | Self::Oklab | Self::OklabAlpha | Self::Scalar
        )
    }
    pub(crate) fn default_cicp(self) -> Cicp {
        match self {
            Self::Cmyk | Self::Scalar => Cicp::UNSPECIFIED,
            Self::Oklab | Self::OklabAlpha => Cicp {
                color_primaries: 1,
                transfer_characteristics: 2,
                matrix_coefficients: 0,
                full_range: true,
            },
            _ => Cicp::SRGB,
        }
    }
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Gray,
            2 => Self::GrayAlpha,
            3 => Self::Rgb,
            4 => Self::Rgba,
            5 => Self::Cmyk,
            6 => Self::Oklab,
            7 => Self::OklabAlpha,
            8 => Self::Scalar,
            _ => return None,
        })
    }
}

/// A sample type (header `sample`, SPEC 1.0). The wire field is open (SPEC
/// 0.3), so the enum is `#[non_exhaustive]`: classify a value with
/// [`Self::bytes`], [`Self::is_float`], [`Self::is_signed`] and
/// [`Self::tier`] instead of an exhaustive match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum SampleType {
    /// 8-bit unsigned.
    U8 = 0,
    /// 16-bit unsigned, native-endian in memory.
    U16 = 1,
    /// IEEE half float bit patterns, native-endian in memory.
    F16 = 2,
    /// 32-bit unsigned, native-endian (extended tier).
    U32 = 3,
    /// 32-bit signed, native-endian (extended tier).
    I32 = 4,
    /// IEEE single float bit patterns, native-endian (extended tier).
    F32 = 5,
    /// 8-bit signed (two's complement, extended tier). Files declare the feature
    /// `signed_samples`.
    I8 = 6,
    /// 16-bit signed (two's complement), native-endian (extended tier). Files declare the
    /// feature `signed_samples`.
    I16 = 7,
    /// 64-bit unsigned, native-endian (extended tier; files declare the
    /// feature `wide_samples`).
    U64 = 8,
    /// 64-bit signed (two's complement), native-endian (extended tier;
    /// files declare the feature `wide_samples`).
    I64 = 9,
    /// IEEE double float bit patterns, native-endian (extended tier; files
    /// declare the feature `wide_samples`).
    F64 = 10,
}

impl SampleType {
    pub fn bytes(self) -> usize {
        match self {
            SampleType::U8 | SampleType::I8 => 1,
            SampleType::U16 | SampleType::F16 | SampleType::I16 => 2,
            SampleType::U32 | SampleType::I32 | SampleType::F32 => 4,
            SampleType::U64 | SampleType::I64 | SampleType::F64 => 8,
        }
    }
    /// Storage bits of one sample.
    pub(crate) fn depth(self) -> u32 {
        8 * self.bytes() as u32
    }
    /// Container depth `D` of one coded channel (SPEC 2.1): the storage
    /// width, except that a 64-bit sample codes two 32-bit words.
    pub(crate) fn word_depth(self) -> u32 {
        self.depth().min(32)
    }
    /// Coded words per sample: 2 for 64-bit samples (hi, lo), else 1.
    pub(crate) fn words(self) -> usize {
        if self.depth() == 64 { 2 } else { 1 }
    }
    /// Whether lossy coding accepts this sample type (u8, u16, u32, f16,
    /// f32). Signed and 64-bit samples are lossless-only.
    pub(crate) fn lossy_capable(self) -> bool {
        matches!(
            self,
            SampleType::U8 | SampleType::U16 | SampleType::U32 | SampleType::F16 | SampleType::F32
        )
    }
    /// Whether the sample is an IEEE float bit pattern (f16, f32, f64).
    pub const fn is_float(self) -> bool {
        matches!(self, SampleType::F16 | SampleType::F32 | SampleType::F64)
    }
    /// Whether the sample is a two's complement signed integer (i8, i16,
    /// i32, i64).
    pub const fn is_signed(self) -> bool {
        matches!(
            self,
            SampleType::I8 | SampleType::I16 | SampleType::I32 | SampleType::I64
        )
    }
    /// Decoder build tier needed for this public sample type.
    /// This is independent of the frozen named wire profiles.
    pub fn tier(self) -> Tier {
        if matches!(self, Self::U8 | Self::U16 | Self::F16) {
            Tier::Baseline
        } else {
            Tier::Extended
        }
    }
    /// Build policy for public image samples, independent of coded-word width.
    pub(crate) fn check_decode_support(self) -> Result<(), Error> {
        if !cfg!(feature = "extended") && self.tier() == Tier::Extended {
            Err(Error::Unsupported(match self.bytes() {
                8 => "64-bit samples (extended tier)",
                4 => "32-bit samples (extended tier)",
                _ => "signed samples (extended tier)",
            }))
        } else {
            Ok(())
        }
    }
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::U8,
            1 => Self::U16,
            2 => Self::F16,
            3 => Self::U32,
            4 => Self::I32,
            5 => Self::F32,
            6 => Self::I8,
            7 => Self::I16,
            8 => Self::U64,
            9 => Self::I64,
            10 => Self::F64,
            _ => return None,
        })
    }
    /// The feature files of this sample type declare (SPEC 1.0):
    /// `signed_samples` for i8/i16, `wide_samples` for 64-bit samples.
    pub(crate) fn required_feature(self) -> Option<crate::parse::header::Feature> {
        use crate::parse::header::Feature;
        match self {
            SampleType::I8 | SampleType::I16 => Some(Feature::SignedSamples),
            SampleType::U64 | SampleType::I64 | SampleType::F64 => Some(Feature::WideSamples),
            _ => None,
        }
    }
    /// Container value -> order-preserving unsigned integer (an involution
    /// for each type; identity for unsigned types). For 64-bit samples this
    /// is the map of the hi word ([`Self::word_order`]).
    #[inline(always)]
    pub(crate) fn order(self, v: u32) -> u32 {
        match self {
            SampleType::F16 => mapping::f16_order(v as u16) as u32,
            SampleType::I8 => v ^ 0x80,
            SampleType::I16 => v ^ 0x8000,
            SampleType::I32 | SampleType::I64 => v ^ 0x8000_0000,
            SampleType::F32 | SampleType::F64 => mapping::f32_order(v),
            _ => v,
        }
    }
}

/// Decoder build tiers. Baseline: u8, u16 and f16. Extended adds all
/// signed integers and 32-/64-bit samples (64-bit samples use two coded words).
/// These build tiers are distinct from historical named wire profiles.
/// A baseline decoder rejects extended sample types
/// with [`Error::Unsupported`]. A later sample type may need a further tier,
/// so the enum is `#[non_exhaustive]`; the tiers are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Tier {
    Baseline,
    Extended,
}

/// Mask of the low `bits` bits (bits <= 32).
#[inline(always)]
pub(crate) fn mask_of(bits: u32) -> u32 {
    ((1u64 << bits) - 1) as u32
}

/// ITU-T H.273 coding-independent code points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cicp {
    pub color_primaries: u8,
    pub transfer_characteristics: u8,
    /// Always 0 (RGB/identity) for this format: samples are not YCbCr.
    pub matrix_coefficients: u8,
    pub full_range: bool,
}

impl Cicp {
    /// Unspecified primaries and transfer, identity matrix, full range.
    pub const UNSPECIFIED: Cicp = Cicp {
        color_primaries: 2,
        transfer_characteristics: 2,
        matrix_coefficients: 0,
        full_range: true,
    };
    /// sRGB: BT.709 primaries, sRGB transfer, full range.
    pub const SRGB: Cicp = Cicp {
        color_primaries: 1,
        transfer_characteristics: 13,
        matrix_coefficients: 0,
        full_range: true,
    };
    /// BT.2100 PQ.
    pub const BT2100_PQ: Cicp = Cicp {
        color_primaries: 9,
        transfer_characteristics: 16,
        matrix_coefficients: 0,
        full_range: true,
    };
    /// BT.2100 HLG.
    pub const BT2100_HLG: Cicp = Cicp {
        color_primaries: 9,
        transfer_characteristics: 18,
        matrix_coefficients: 0,
        full_range: true,
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub channels: Channels,
    pub sample: SampleType,
    pub cicp: Cicp,
    /// Interpretation of the stored final slot in GrayAlpha/Rgba/OklabAlpha images.
    /// Lossless coding preserves all slot bytes, including undefined padding.
    /// Straight and premultiplied retain their original wire representation.
    pub alpha_mode: AlphaMode,
    /// Right-aligned significant integer bits, in memory channel order.
    /// Zero means the complete storage width; unused entries must be zero.
    /// Lossless coding preserves every storage bit, including padding. Lossy
    /// coding requires unsigned samples within the declared range and returns
    /// that same code grid. Floats always use their complete representation.
    /// Prefer [`Self::with_significant_bits`] to construct this declaration.
    pub significant_bits: [u8; 4],
}

impl ImageInfo {
    pub fn new(width: u32, height: u32, channels: Channels, sample: SampleType) -> Self {
        Self {
            width,
            height,
            channels,
            sample,
            cicp: channels.default_cicp(),
            alpha_mode: AlphaMode::Straight,
            significant_bits: [0; 4],
        }
    }
    /// Declares each channel's right-aligned integer code range. This does
    /// not truncate samples or describe color-transform intermediate widths.
    pub fn with_significant_bits(mut self, bits: &[u8]) -> Result<Self, Error> {
        if bits.len() != self.channels.count() || bits.contains(&0) {
            return Err(Error::InvalidInput(
                "significant-bit channel count or zero width",
            ));
        }
        self.significant_bits = [0; 4];
        for (out, &n) in self.significant_bits.iter_mut().zip(bits) {
            *out = if n as u32 == self.sample.depth() {
                0
            } else {
                n
            };
        }
        self.validate_precision()?;
        Ok(self)
    }
    #[cfg_attr(not(any(feature = "encode", feature = "std")), allow(dead_code))]
    pub(crate) fn alpha_wire(&self) -> Result<u8, Error> {
        let value = match self.alpha_mode {
            AlphaMode::Straight => 0,
            AlphaMode::Premultiplied => 1,
            AlphaMode::Opaque => 2,
            AlphaMode::Undefined => 3,
            _ => return Err(Error::Unsupported("alpha interpretation")),
        };
        if value >= 2 && !self.channels.has_alpha() {
            return Err(Error::InvalidInput(
                "alpha interpretation needs a stored alpha slot",
            ));
        }
        Ok(value)
    }
    /// Declared bits for a channel, or None for an unused channel.
    pub fn channel_bits(&self, channel: usize) -> Option<u8> {
        (channel < self.channels.count()).then(|| {
            let n = self.significant_bits[channel];
            if n == 0 { self.sample.depth() as u8 } else { n }
        })
    }
    pub(crate) fn has_custom_precision(&self) -> bool {
        (0..self.channels.count()).any(|c| self.channel_bits(c) != Some(self.sample.depth() as u8))
    }
    pub(crate) fn validate_precision(&self) -> Result<(), Error> {
        let float = self.sample.is_float();
        for (c, &n) in self.significant_bits.iter().enumerate() {
            if (c >= self.channels.count() && n != 0)
                || n as u32 > self.sample.depth()
                || (float && n != 0 && n as u32 != self.sample.depth())
            {
                return Err(Error::InvalidInput("invalid significant-bit declaration"));
            }
        }
        Ok(())
    }
    /// Bytes per pixel in memory.
    pub fn bytes_per_pixel(&self) -> usize {
        self.channels.count() * self.sample.bytes()
    }
    /// Tightly packed buffer size, if it fits in `usize`.
    pub fn checked_buffer_len(&self) -> Option<usize> {
        usize::try_from(self.checked_buffer_len_u64()?).ok()
    }
    /// Tightly packed buffer size, if it fits in `u64`. What a reader checks
    /// before the host's pointer width enters: on a 32-bit host a huge but
    /// well-formed header still parses, and a route that then allocates
    /// reports `TooLarge` from its own `usize` check.
    pub(crate) fn checked_buffer_len_u64(&self) -> Option<u64> {
        let row = (self.width as u64).checked_mul(self.bytes_per_pixel() as u64)?;
        row.checked_mul(self.height as u64)
    }
    /// Extent through the last real sample, excluding final-row padding.
    pub(crate) fn checked_stride_len(&self, stride: usize) -> Option<usize> {
        let row = (self.width as usize).checked_mul(self.bytes_per_pixel())?;
        if row == 0 || stride < row {
            return None;
        }
        stride
            .checked_mul((self.height as usize).checked_sub(1)?)?
            .checked_add(row)
    }
    /// Tightly packed row length in bytes.
    pub fn row_bytes(&self) -> usize {
        self.width as usize * self.bytes_per_pixel()
    }
}

/// Image/output limits, not an aggregate memory or work budget.
///
/// Scratch, retained metadata, frame count and concurrent decodes need separate
/// application limits. The reader also uses `max_bytes` as a per-read bound.
///
/// New fields can arrive in any release: start from [`Limits::DEFAULT`],
/// [`Limits::NONE`] or [`Limits::default`] and assign the fields to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum `width * height`.
    pub max_pixels: u64,
    /// Maximum decoded buffer size in bytes.
    pub max_bytes: u64,
    /// Check a file checksum (`cSUM` chunk, SPEC 1.4) when the file has one.
    /// `false` treats the chunk and its header record as a decoder without
    /// checksums would: skipped.
    pub verify_checksum: bool,
}

impl Limits {
    /// 120 megapixels, 1 GiB of output; checksums verified.
    pub const DEFAULT: Limits = Limits {
        max_pixels: 120_000_000,
        max_bytes: 1 << 30,
        verify_checksum: true,
    };
    /// No limits beyond address-space arithmetic (trusted input only);
    /// checksums verified.
    pub const NONE: Limits = Limits {
        max_pixels: u64::MAX,
        max_bytes: u64::MAX,
        verify_checksum: true,
    };

    /// Sets [`Self::verify_checksum`].
    pub const fn with_checksum_verification(mut self, verify: bool) -> Self {
        self.verify_checksum = verify;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Encoder target for the decoder's aggregate codec working buffers.
///
/// Planning covers `StreamDecoder` and slice decoding with up to `threads`
/// concurrent workers. It excludes compressed input, caller/output buffers,
/// metadata, reader I/O buffering, thread stacks and allocator/runtime overhead.
/// This is a conservative encoder estimate, not a decoder-enforced allocation
/// cap or a process RSS limit. Use application limits for untrusted files.
///
/// A budget may override stripe/group/lane/palette preferences, but never
/// changes quantization, color, or lossless alpha. No budget is set by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecoderMemoryLimit {
    /// Maximum estimated working-buffer bytes for one image decode.
    pub max_bytes: u64,
    /// Maximum simultaneous decoder workers, independent of encoder threads.
    /// Must be at least one, even on an encoding host without threading.
    pub threads: usize,
}

impl DecoderMemoryLimit {
    /// A budget for single-threaded decoding and row streaming.
    pub const fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            threads: 1,
        }
    }

    pub const fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }
}

/// Optional metadata blocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub icc: Option<Vec<u8>>,
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<Vec<u8>>,
    /// SMPTE ST 2086 mastering display color volume, as the 24-byte
    /// big-endian payload used by PNG `mDCV` / ISOBMFF `mdcv`.
    pub mastering_display: Option<[u8; 24]>,
    /// Content light level (MaxCLL, MaxFALL): two big-endian `u32` values,
    /// each in 0.0001 cd/m² units, matching PNG third-edition `cLLI`.
    /// Zero means unknown. Readers convert legacy four-byte whole-nit
    /// records into these units; writers always emit eight bytes.
    pub content_light_level: Option<[u8; 8]>,
    /// Absolute luminance of relative-linear white, in cd/m².
    /// Stored bit-exactly in `dWHI`; absence stays distinct from a renderer default.
    pub diffuse_white: Option<zenpixels::hdr::DiffuseWhite>,
    /// EXIF orientation 1..=8 (0 is written as 1). Stored in the header;
    /// decoders return pixels as stored.
    pub orientation: u8,
    /// ISO 21496-1 gain map.
    pub gain_map: Option<GainMap>,
    /// Ancillary chunks kept in file order. Typed optional records include
    /// [`Metadata::color_origin`] and [`item_metadata::ItemMetadata`].
    pub chunks: Vec<Chunk>,
}

/// A codec error. `#[non_exhaustive]`: a later version may add a class;
/// match the ones you handle and keep a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The input is not an IMI file.
    NotImij,
    /// The file is malformed.
    Corrupt(&'static str),
    /// Caller-supplied buffer or parameters are invalid.
    InvalidInput(&'static str),
    /// Image exceeds limits.
    TooLarge,
    /// A caller-owned encode or decode operation requested cancellation.
    Cancelled,
    /// The file needs a feature this decoder was built without.
    Unsupported(&'static str),
    /// The reader passed to `decode_reader` returned an error.
    Read,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotImij => f.write_str("not an IMI file"),
            Error::Corrupt(s) => write!(f, "corrupt IMI data: {s}"),
            Error::InvalidInput(s) => write!(f, "invalid input: {s}"),
            Error::TooLarge => f.write_str("image too large"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Read => f.write_str("read error"),
        }
    }
}

impl core::error::Error for Error {
    /// A refusal named after a zencodec operation (`Unsupported` holding that
    /// operation's `name()`) exposes the typed operation, so
    /// `zencodec::CodecErrorExt::unsupported_operation` identifies it.
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Error::Unsupported(name) => codec::operation(name).map(|op| op as _),
            _ => None,
        }
    }
}

impl how_far::AsStopReason for Error {
    fn as_stop_reason(&self) -> Option<how_far::StopReason> {
        matches!(self, Self::Cancelled).then_some(how_far::StopReason::Cancelled)
    }
}

impl Error {
    /// Whether the input is malformed: [`Error::NotImij`] or
    /// [`Error::Corrupt`] (SPEC 0.3: a closed value or gated syntax without
    /// its feature). A newer decoder rejects such a file too.
    pub const fn is_malformed(&self) -> bool {
        matches!(self, Error::NotImij | Error::Corrupt(_))
    }

    /// Whether the file needs something this decoder does not implement
    /// ([`Error::Unsupported`]: a feature, an open value, an unknown critical
    /// record or chunk). A newer or fuller build may decode it.
    pub const fn is_unsupported(&self) -> bool {
        matches!(self, Error::Unsupported(_))
    }
}

// ---- small byte helpers ----------------------------------------------------

pub(crate) fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Bytes [`put_varint`] writes for `v`.
pub(crate) fn varint_len(v: u64) -> usize {
    ((64 - v.leading_zeros()).max(1) as usize).div_ceil(7)
}

/// u64 LEB128 shared by slices and readers, minimal encoding only: a
/// multi-byte varint whose last byte is zero is corrupt (SPEC section 0), so
/// every value has exactly one encoding. The tenth byte may only contain the
/// remaining high bit.
fn read_varint(mut byte: impl FnMut() -> Result<u8, Error>) -> Result<u64, Error> {
    let mut value = 0u64;
    for i in 0..10 {
        let b = byte()?;
        if i == 9 && b > 1 {
            return Err(Error::Corrupt("varint overflow"));
        }
        value |= ((b & 0x7f) as u64) << (7 * i);
        if b < 0x80 {
            if b == 0 && i > 0 {
                return Err(Error::Corrupt("non-minimal varint"));
            }
            return Ok(value);
        }
    }
    Err(Error::Corrupt("bad varint"))
}

pub(crate) struct ByteReader<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    /// Address of the next byte, for the lab codestream trace.
    #[cfg_attr(not(all(feature = "lab", feature = "std")), allow(dead_code))]
    pub(crate) fn addr(&self) -> usize {
        self.data.as_ptr() as usize + self.pos
    }
    /// This reader's position for the trace.
    pub(crate) fn mark(&self) -> crate::trace::Mark {
        crate::trace::Mark::Addr(self.addr())
    }
    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(Error::Corrupt("length overflow"))?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or(Error::Corrupt("truncated"))?;
        self.pos = end;
        Ok(s)
    }
    /// `bytes` for a length read from the stream (`u64`): the bounds checks
    /// run before the pointer-width narrowing, so a 32-bit reader reports
    /// the same outcome as a 64-bit one on a length past the input.
    pub(crate) fn bytes_len(&mut self, n: u64) -> Result<&'a [u8], Error> {
        let end = (self.pos as u64)
            .checked_add(n)
            .ok_or(Error::Corrupt("length overflow"))?;
        let end = usize::try_from(end).map_err(|_| Error::Corrupt("truncated"))?;
        let s = self
            .data
            .get(self.pos..end)
            .ok_or(Error::Corrupt("truncated"))?;
        self.pos = end;
        Ok(s)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub(crate) fn varint(&mut self) -> Result<u64, Error> {
        read_varint(|| self.u8())
    }
    pub(crate) fn var_usize(&mut self) -> Result<usize, Error> {
        usize::try_from(self.varint()?).map_err(|_| Error::TooLarge)
    }
    pub(crate) fn var_u32(&mut self) -> Result<u32, Error> {
        u32::try_from(self.varint()?).map_err(|_| Error::Corrupt("integer exceeds u32"))
    }
}

#[cfg(all(test, feature = "encode"))]
mod varint_tests {
    use super::{ByteReader, Error, put_varint};

    fn read(bytes: &[u8]) -> Result<u64, Error> {
        let mut r = ByteReader::new(bytes);
        let v = r.varint()?;
        assert_eq!(r.remaining(), 0, "varint must consume exactly its bytes");
        Ok(v)
    }

    #[test]
    fn minimal_round_trip_at_every_group_boundary() {
        let mut values = vec![0u64, 1, u64::MAX, u64::MAX - 1];
        for k in 1..64 {
            values.extend([(1u64 << k) - 1, 1u64 << k, (1u64 << k) + 1]);
        }
        for v in values {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            let bits = 64 - v.leading_zeros().min(63);
            assert_eq!(b.len() as u32, bits.div_ceil(7).max(1), "{v}");
            assert_eq!(read(&b), Ok(v), "{v}");
        }
    }

    #[test]
    fn non_minimal_overflowing_and_truncated_varints_are_rejected() {
        let corrupt = |b: &[u8], why| assert_eq!(read(b), Err(Error::Corrupt(why)), "{b:02x?}");
        corrupt(&[0x80, 0x00], "non-minimal varint");
        corrupt(&[0xff, 0x80, 0x00], "non-minimal varint");
        corrupt(
            &[0x80; 9].iter().copied().chain([0x00]).collect::<Vec<_>>(),
            "non-minimal varint",
        );
        // The tenth byte holds only bit 63.
        corrupt(
            &[0xff; 9].iter().copied().chain([0x02]).collect::<Vec<_>>(),
            "varint overflow",
        );
        corrupt(&[0x80; 10], "varint overflow");
        assert!(read(&[0x80]).is_err());
        assert_eq!(read(&[0x00]), Ok(0));
    }
}
