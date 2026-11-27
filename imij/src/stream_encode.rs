//! One-pass, bounded-group lossless encoding with caller-driven output.
//!
//! [`GroupEncodeSession`] works without I/O or `std`; `GroupEncoder` is
//! its `std::io::Write` adapter. Integer and floating sample bits are preserved.
//! It uses
//! identity maps, fixed positive entropy tables and serial rectangular groups.
//! It trades compression for bounded working storage; it does not reproduce
//! the whole-image encoder's bytes. The basic session accumulates no index or
//! checksum. `IndexedGroupEncoder` spools a trailing index to caller-owned
//! seekable scratch (`std` only), retaining no per-group list in memory.
//! [`encode_forward_rows_indexed`] accepts a forward-only raster with one-row
//! column groups. [`encode_forward_bands_indexed`] uses caller-owned seekable
//! band scratch to feed taller groups. Source, index-scratch and writer
//! buffering remain explicit caller responsibilities.

#[cfg(feature = "std")]
mod append;
#[cfg(feature = "std")]
pub(crate) mod checked;
#[cfg(feature = "std")]
mod forward;
#[cfg(feature = "std")]
mod growing;
#[cfg(feature = "std")]
mod indexed;
#[cfg(feature = "std")]
pub(crate) mod lossy;
#[cfg(feature = "std")]
pub(crate) mod spool;
#[cfg(feature = "std")]
pub use append::AppendRowsEncoder;
#[cfg(feature = "std")]
pub use checked::{ChecksummedGroupConfig, ChecksummedGroupEncoder};
#[cfg(feature = "std")]
pub use forward::{
    ForwardBandConfig, ForwardRowConfig, encode_forward_bands_indexed, encode_forward_rows_indexed,
};
#[cfg(feature = "std")]
pub use growing::GrowingHeightGroupEncoder;
#[cfg(feature = "std")]
pub use indexed::IndexedGroupEncoder;
#[cfg(feature = "std")]
pub use lossy::{
    BoundedLossyConfig, BoundedLossyPasses, BoundedLossyPlan, BoundedLossySession,
    BoundedLossyStep, BoundedStream, LossyUnitIndex, encode_lossy_bounded,
};

pub use crate::v4::GroupEncodeSession;

use crate::bits::BitWriter;
use crate::coding_layout::Layout;
use crate::encode::AnalysisConfig;
use crate::mapping::ChannelMap;
use crate::rans::{self, Model, NCTX, NL};
use crate::tokens::TokenCfg;
use crate::{Error, ImageInfo, Metadata, put_varint};
use alloc::{vec, vec::Vec};
#[cfg(feature = "std")]
use std::io::Write;

/// Group geometry and conservative codec working-storage admission.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GroupEncodeConfig {
    /// Maximum columns in each independent rectangle.
    pub columns: u32,
    /// Maximum rows in each independent rectangle.
    pub rows: u32,
    /// Codec working-storage allowance, excluding source/sink buffers and
    /// allocator overhead. Admission is conservative, not a measured RSS cap.
    pub max_working_bytes: usize,
    /// Use exact quick-pixel packets instead of fixed MED/rANS groups.
    pub quick_pixels: bool,
    /// Exact byte-plane preprocessing for quick groups.
    pub quick_bit_smarts: crate::QuickBitSmarts,
    /// Per-channel error bound in sample units (`[0; 4]` = exact lossless).
    pub max_error: [u32; 4],
}

impl GroupEncodeConfig {
    /// Select quick groups with explicit exact bit preprocessing.
    pub(crate) const fn with_quick_bit_smarts(mut self, bits: crate::QuickBitSmarts) -> Self {
        self.quick_pixels = true;
        self.quick_bit_smarts = bits;
        self
    }
    /// Construct an explicit memory and geometry policy.
    pub(crate) const fn new(columns: u32, rows: u32, max_working_bytes: usize) -> Self {
        Self {
            columns,
            rows,
            max_working_bytes,
            quick_pixels: false,
            quick_bit_smarts: crate::QuickBitSmarts::Off,
            max_error: [0; 4],
        }
    }
}

/// The exact source rectangle requested next, in stripe-major/group order.
impl GroupEncodeConfig {
    /// The group settings of a configuration that must have row groups.
    pub(crate) fn required(cfg: &crate::ImiEncoderConfig) -> Result<Self, Error> {
        let groups = cfg
            .row_groups()
            .ok_or(Error::InvalidInput("row groups required"))?;
        Self::from_config(cfg, groups)
    }

    /// The group settings of a public configuration: its [`RowGroups`] with
    /// the quick coder when the coding is `Quick`.
    pub(crate) fn from_config(
        cfg: &crate::ImiEncoderConfig,
        groups: crate::RowGroups,
    ) -> Result<Self, Error> {
        let mut out = Self::new(groups.columns, groups.rows, groups.max_working_bytes);
        if let Some(tuning) = cfg.lossless_tuning() {
            out.max_error = tuning.max_error;
        }
        match cfg.coder() {
            crate::Coder::Lossless => {}
            crate::Coder::Quick(bits) => out = out.with_quick_bit_smarts(bits),
            crate::Coder::Lossy(_) => {
                return Err(Error::Unsupported(
                    "row groups code losslessly; lossy streaming is the bounded job",
                ));
            }
        }
        Ok(out)
    }
}

/// Encodes an in-memory image through a row-group session: the same file a
/// caller-driven [`GroupEncodeSession`] writes, collected into one buffer.
pub(crate) fn encode_row_groups(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &crate::ImiEncoderConfig,
    groups: crate::RowGroups,
    pulse: &dyn how_far::Pulse,
) -> Result<Vec<u8>, Error> {
    let group_cfg = GroupEncodeConfig::from_config(cfg, groups)?;
    let bpp = info.bytes_per_pixel();
    let mut session = GroupEncodeSession::with_groups(info.clone(), meta, group_cfg)?;
    let mut out = Vec::new();
    loop {
        // The initial header, and then every completed group, must be
        // transferred before the session can request the next rectangle.
        let count = session.output().len();
        out.try_reserve(count).map_err(|_| Error::TooLarge)?;
        out.extend_from_slice(session.output());
        session.consume_output(count)?;
        let Some(rect) = session.next_group() else {
            break;
        };
        pulse.check().map_err(|_| Error::Cancelled)?;
        let start = (rect.y as usize)
            .checked_mul(stride)
            .and_then(|o| o.checked_add((rect.x as usize).checked_mul(bpp)?))
            .ok_or(Error::TooLarge)?;
        let rows = rect.height as usize;
        let end = if rows == 0 {
            start
        } else {
            start
                .checked_add((rows - 1).checked_mul(stride).ok_or(Error::TooLarge)?)
                .and_then(|o| o.checked_add((rect.width as usize).checked_mul(bpp)?))
                .ok_or(Error::TooLarge)?
        };
        let slice = pixels
            .get(start..end)
            .ok_or(Error::InvalidInput("pixel buffer too small"))?;
        session.push_group(slice, stride)?;
    }
    session.finish()?;
    crate::checksum::finish_file(out, cfg.checksum_algorithm())
}

pub(crate) fn encode_legacy_row_groups(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &crate::ImiEncoderConfig,
    groups: crate::RowGroups,
    pulse: &dyn how_far::Pulse,
) -> Result<Vec<u8>, Error> {
    let group_cfg = GroupEncodeConfig::from_config(cfg, groups)?;
    let bpp = info.bytes_per_pixel();
    let mut session = NativeGroupEncodeSession::with_groups(info.clone(), meta, group_cfg)?;
    let mut out = Vec::new();
    loop {
        // The initial header, and then every completed group, must be
        // transferred before the session can request the next rectangle.
        let count = session.output().len();
        out.try_reserve(count).map_err(|_| Error::TooLarge)?;
        out.extend_from_slice(session.output());
        session.consume_output(count)?;
        let Some(rect) = session.next_group() else {
            break;
        };
        pulse.check().map_err(|_| Error::Cancelled)?;
        let start = (rect.y as usize)
            .checked_mul(stride)
            .and_then(|o| o.checked_add((rect.x as usize).checked_mul(bpp)?))
            .ok_or(Error::TooLarge)?;
        let rows = rect.height as usize;
        let end = if rows == 0 {
            start
        } else {
            start
                .checked_add((rows - 1).checked_mul(stride).ok_or(Error::TooLarge)?)
                .and_then(|o| o.checked_add((rect.width as usize).checked_mul(bpp)?))
                .ok_or(Error::TooLarge)?
        };
        let slice = pixels
            .get(start..end)
            .ok_or(Error::InvalidInput("pixel buffer too small"))?;
        session.push_group(slice, stride)?;
    }
    session.finish()?;
    crate::checksum::finish_file(out, cfg.checksum_algorithm())
}

#[cfg(all(test, feature = "encode", feature = "std"))]
mod row_group_adapter_tests {
    use super::*;

    #[test]
    fn resident_row_groups_drain_header_and_match_the_stream_writer() {
        for (width, height) in [(1, 1), (65, 17)] {
            for channels in [crate::Channels::Gray, crate::Channels::Rgb] {
                for sample in [crate::SampleType::U8, crate::SampleType::U16] {
                    let mut info = ImageInfo::new(width, height, channels, sample);
                    let bits = if sample == crate::SampleType::U8 {
                        8
                    } else {
                        10
                    };
                    info = info
                        .with_significant_bits(&vec![bits; channels.count()])
                        .unwrap();
                    let row = width as usize * info.bytes_per_pixel();
                    let stride = row + 9;
                    let mut pixels = vec![0xa5; stride * (height as usize - 1) + row];
                    let mut expected = Vec::new();
                    for y in 0..height as usize {
                        for x in 0..width as usize * channels.count() {
                            let code = ((x * 181 + y * 73) & ((1 << bits) - 1)) as u16;
                            let (bytes, count) = if sample == crate::SampleType::U8 {
                                ([code as u8, 0], 1)
                            } else {
                                (code.to_ne_bytes(), 2)
                            };
                            let value = &bytes[..count];
                            let at = y * stride + x * value.len();
                            pixels[at..at + value.len()].copy_from_slice(value);
                            expected.extend_from_slice(value);
                        }
                    }
                    let cfg = crate::ImiEncoderConfig::lossless()
                        .with_row_groups(crate::RowGroups::new(32, 8, 16 << 20));
                    let metadata = Metadata::default();
                    let encoded = cfg
                        .encode_samples(crate::ImageRef::new(&pixels, stride, &info), &metadata)
                        .unwrap();
                    let mut streamed = Vec::new();
                    let mut writer =
                        GroupEncoder::new(&mut streamed, info.clone(), &metadata, &cfg).unwrap();
                    while let Some(rect) = writer.next_group() {
                        let at =
                            rect.y as usize * stride + rect.x as usize * info.bytes_per_pixel();
                        writer.write_group(&pixels[at..], stride).unwrap();
                    }
                    writer.finish().unwrap();
                    assert_eq!(encoded, streamed);
                    let (decoded_info, decoded) = crate::decode(&encoded).unwrap();
                    assert_eq!(decoded_info, info);
                    assert_eq!(decoded, expected);
                }
            }
        }
    }

    #[test]
    fn resident_row_groups_honor_requested_file_checksums() {
        let info = ImageInfo::new(65, 17, crate::Channels::Gray, crate::SampleType::U8);
        let pixels: Vec<_> = (0..65 * 17).map(|n| (n * 73) as u8).collect();
        for algorithm in [
            crate::HashAlgorithm::Xxh3_64,
            crate::HashAlgorithm::Xxh3_128,
        ] {
            let cfg = crate::ImiEncoderConfig::lossless()
                .with_row_groups(crate::RowGroups::new(32, 8, 16 << 20))
                .with_checksum(true)
                .with_hash_algorithm(algorithm);
            let encoded = cfg
                .encode_legacy_samples(
                    crate::ImageRef::new(&pixels, 65, &info),
                    &Metadata::default(),
                )
                .unwrap();
            let parsed =
                crate::parse::header::read_header_sized(&mut crate::ByteReader::new(&encoded))
                    .unwrap();
            assert_eq!(
                parsed.checksum.unwrap().announced.algorithm(),
                Some(algorithm)
            );
            assert_eq!(crate::decode(&encoded).unwrap().1, pixels);
            let mut changed = encoded;
            *changed.last_mut().unwrap() ^= 1;
            assert!(matches!(
                crate::decode(&changed),
                Err(Error::Corrupt("file checksum mismatch"))
            ));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GroupRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Codec/admission or caller-writer failure. Partial output must be discarded
/// after a write failure; the encoder cannot resume it.
#[cfg(feature = "std")]
#[derive(Debug)]
#[non_exhaustive]
pub enum StreamEncodeError {
    Codec(Error),
    Read(std::io::Error),
    Write(std::io::Error),
}
#[cfg(feature = "std")]
impl From<Error> for StreamEncodeError {
    fn from(e: Error) -> Self {
        Self::Codec(e)
    }
}
#[cfg(feature = "std")]
impl From<std::io::Error> for StreamEncodeError {
    fn from(e: std::io::Error) -> Self {
        Self::Write(e)
    }
}
#[cfg(feature = "std")]
impl core::fmt::Display for StreamEncodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Codec(e) => e.fmt(f),
            Self::Read(e) => e.fmt(f),
            Self::Write(e) => e.fmt(f),
        }
    }
}
#[cfg(feature = "std")]
impl std::error::Error for StreamEncodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Codec(e) => e,
            Self::Read(e) => e,
            Self::Write(e) => e,
        })
    }
}

/// Encodes a single lossless image incrementally, retaining no image-sized
/// analysis, unit list or encoded output. The existing lossless wire and
/// decoders are used unchanged. Indexed ROI access needs a separate index.
///
/// Drain [`Self::output`] with [`Self::consume_output`] before supplying
/// [`Self::next_group`] to [`Self::push_group`]. A partial drain retains exactly
/// the unconsumed suffix; no further input is accepted until it is drained.
/// Finally call [`Self::finish`]. All storage bits are preserved, including
/// NaN payloads, significant-bit padding and exact alpha. Output is bounded by
/// one header or one encoded group; the session never accumulates the image.
#[derive(Default)]
struct GroupScratch {
    planes: Vec<Vec<u32>>,
    residuals: Vec<Vec<u32>>,
    streams: Vec<Vec<u8>>,
    symbols: [Vec<rans::Sym>; NL],
    rans: rans::RansScratch,
}

pub(crate) struct NativeGroupEncodeSession {
    output: Vec<u8>,
    consumed: usize,
    info: ImageInfo,
    cfg: GroupEncodeConfig,
    layout: Layout,
    model: Option<Model>,
    /// Per-slot models measured by the caller (nested DC images of bounded
    /// lossy encoding); `None` codes every slot with `model`.
    slot_models: Option<Vec<Model>>,
    /// rANS groups' analysis: decorrelation and predictor masks.
    analysis: (u8, u8),
    /// One shared fixed-model table, or one per learned nested DC slot.
    /// Tables are built once and reused for every group.
    tables: Vec<rans::EncodeTables<'static>>,
    scratch: GroupScratch,
    x: u32,
    y: u32,
}

impl NativeGroupEncodeSession {
    pub(crate) fn with_groups(
        info: ImageInfo,
        meta: &Metadata,
        cfg: GroupEncodeConfig,
    ) -> Result<Self, Error> {
        Self::new_inner(info, meta, cfg, false).map(|(session, _)| session)
    }

    /// A nested lossless image coded group by group (the bounded lossy
    /// encoder's DC image): the DATA head an outer payload embeds, and a
    /// session with the file header already consumed. Each pushed group's
    /// output is one `R_STRIPE`-framed stripe record.
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub(crate) fn nested(
        info: ImageInfo,
        cfg: GroupEncodeConfig,
    ) -> Result<(Self, Vec<u8>), Error> {
        let (mut session, data_len) =
            Self::new_rows(info, &Metadata::default(), cfg, false, cfg.rows, None)?;
        let at = session.output.len() - data_len as usize;
        let head = session.output[at..].to_vec();
        session.consumed = session.output.len();
        Ok((session, head))
    }

    // Share group admission with the unknown-height writer. Its provisional
    // one-row header must not shrink the maximum group it will later accept.
    pub(crate) fn check_working(
        info: &ImageInfo,
        meta: &Metadata,
        cfg: GroupEncodeConfig,
    ) -> Result<(), Error> {
        if info.width == 0 || info.height == 0 || cfg.columns == 0 || cfg.rows == 0 {
            return Err(Error::InvalidInput("group geometry must be nonzero"));
        }
        info.validate_precision()?;
        if !cfg.quick_pixels && cfg.quick_bit_smarts != crate::QuickBitSmarts::Off {
            return Err(Error::InvalidInput("bit smarts require quick groups"));
        }
        let near_requested = cfg.max_error[..info.channels.count()]
            .iter()
            .any(|&d| d != 0);
        if near_requested {
            if cfg.quick_pixels {
                return Err(Error::Unsupported(
                    "near-lossless is not supported with quick groups",
                ));
            }
            if !crate::near::defined_for(info.sample) {
                return Err(Error::InvalidInput(if info.sample.is_float() {
                    "near-lossless needs integer samples (quantize floats to integers first)"
                } else {
                    "near-lossless coding is defined for u8, u16, u32 and i32 samples"
                }));
            }
        }
        let columns = cfg.columns.min(info.width) as usize;
        let rows = cfg.rows.min(info.height) as usize;
        if columns.checked_mul(rows).ok_or(Error::TooLarge)? > u32::MAX as usize {
            return Err(Error::TooLarge);
        }
        // Padding to sixteen lanes, live input/residual/symbol/renormalization
        // vectors and their growth capacities, plus fixed model/header space.
        // No term depends on the total image or the number of units.
        let group = if cfg.quick_pixels {
            columns
                .checked_mul(rows)
                .and_then(|n| n.checked_mul(info.bytes_per_pixel() + 1))
                .and_then(|n| {
                    n.checked_mul(if cfg.quick_bit_smarts == crate::QuickBitSmarts::Off {
                        4
                    } else {
                        12
                    })
                })
                .ok_or(Error::TooLarge)?
        } else {
            columns
                .checked_add(15)
                .and_then(|w| w.checked_mul(rows))
                .and_then(|n| n.checked_mul(info.channels.count()))
                .and_then(|n| n.checked_mul(info.sample.words()))
                .and_then(|n| n.checked_mul(512))
                .ok_or(Error::TooLarge)?
        };
        let mut metadata = 4096usize;
        let mut add = |n: usize| -> Result<(), Error> {
            metadata = metadata.checked_add(n).ok_or(Error::TooLarge)?;
            Ok(())
        };
        for value in [&meta.icc, &meta.exif, &meta.xmp].into_iter().flatten() {
            add(value.len())?;
        }
        for c in &meta.chunks {
            add(c.data.len().checked_add(32).ok_or(Error::TooLarge)?)?;
        }
        if let Some(g) = &meta.gain_map {
            add(g.image.len())?;
            if let Some(icc) = &g.alt_icc {
                add(icc.len())?;
            }
        }
        let needed = metadata
            .checked_mul(4)
            .and_then(|n| n.checked_add(group))
            .and_then(|n| n.checked_add(1 << 20))
            .ok_or(Error::TooLarge)?;
        if needed > cfg.max_working_bytes {
            return Err(Error::TooLarge);
        }
        Ok(())
    }

    fn new_inner(
        info: ImageInfo,
        meta: &Metadata,
        cfg: GroupEncodeConfig,
        indexed: bool,
    ) -> Result<(Self, u64), Error> {
        let rows = cfg.rows.min(info.height);
        Self::new_rows(info, meta, cfg, indexed, rows, None)
    }

    /// [`Self::new_inner`] writing `head_rows` as the stripe height (a nested
    /// image states its outer payload's stripe height even past its own end).
    fn new_rows(
        info: ImageInfo,
        meta: &Metadata,
        cfg: GroupEncodeConfig,
        indexed: bool,
        head_rows: u32,
        slot_models: Option<Vec<Model>>,
    ) -> Result<(Self, u64), Error> {
        Self::check_working(&info, meta, cfg)?;
        let maps = vec![
            ChannelMap::identity(info.sample.word_depth());
            info.channels.count() * info.sample.words()
        ];
        let mut near = vec![0u32; maps.len()];
        let near_requested = !cfg.quick_pixels
            && cfg.max_error[..info.channels.count()]
                .iter()
                .any(|&d| d != 0);
        if near_requested {
            let depth = info.sample.depth();
            for c in 0..info.channels.count() {
                near[c] = crate::near::coded_delta(&maps[c], depth, cfg.max_error[c]);
            }
        }
        let layout = Layout::new(info.channels, maps).with_near(&near);
        let near_used = layout.is_near();
        let has_records = indexed || near_used;
        let model = if cfg.quick_pixels {
            None
        } else {
            Some(fixed_group_model(&info)?)
        };
        let mut head = Vec::new();
        let features = if cfg.quick_pixels {
            crate::quick::features(cfg.quick_bit_smarts)
        } else if near_used {
            crate::parse::header::Features::EMPTY.with(crate::Feature::NearLossless)
        } else {
            crate::parse::header::Features::EMPTY
        };
        crate::container::write_group_head_features(
            &mut head,
            &info,
            meta,
            crate::Coding::Lossless,
            None,
            crate::DecodeSpeed::Balanced,
            features,
            crate::parse::header::HEADER_VERSION,
        )?;
        head.extend_from_slice(b"DATA\0"); // open DATA: unit count comes from geometry
        let data_at = head.len();
        put_varint(
            &mut head,
            (if has_records { crate::v1::H_RECORDS } else { 0 })
                | if cfg.quick_pixels {
                    crate::v1::H_QUICK
                        | if cfg.quick_bit_smarts == crate::QuickBitSmarts::Off {
                            0
                        } else {
                            crate::v1::H_QUICK_BITS
                        }
                } else {
                    0
                },
        );
        head.push(if cfg.quick_pixels { 1 } else { 16 });
        put_varint(&mut head, head_rows as u64);
        put_varint(
            &mut head,
            if cfg.columns >= info.width {
                0
            } else {
                cfg.columns as u64
            },
        );
        if !cfg.quick_pixels {
            for m in &layout.maps {
                head.extend_from_slice(&[m.mode as u8, m.bits, 0, 0]);
                if info.sample.bytes() >= 4 {
                    head.extend_from_slice(&[0, 0]);
                }
            }
            let mut bits = BitWriter::new();
            for s in 0..layout.slot_channel.len() {
                match &slot_models {
                    Some(m) => m[s].write(&mut bits, None),
                    None => model
                        .as_ref()
                        .expect("rANS group model")
                        .write(&mut bits, None),
                }
            }
            let bytes = bits.finish();
            put_varint(&mut head, bytes.len() as u64);
            head.extend_from_slice(&bytes);
        }
        if has_records {
            let mut records = Vec::new();
            if indexed {
                crate::parse::records::put_record(
                    &mut records,
                    crate::parse::records::H_UNIT_RESERVE,
                    &[crate::parse::records::RESERVE_TAIL],
                );
            }
            if near_used {
                let mut bounds = Vec::new();
                for d in layout.channel_near() {
                    put_varint(&mut bounds, d as u64);
                }
                crate::parse::records::put_record(&mut records, crate::v1::H_NEAR, &bounds);
            }
            put_varint(&mut head, records.len() as u64);
            head.extend_from_slice(&records);
        }
        let data_len = (head.len() - data_at) as u64;
        let tables = if cfg.quick_pixels {
            Vec::new()
        } else {
            // The fixed model is identical across slots; learned nested DC
            // models retain their individual tables.
            let count = if slot_models.is_some() {
                layout.slot_channel.len()
            } else {
                1
            };
            (0..count)
                .map(|s| {
                    rans::EncodeTables::per_class(match &slot_models {
                        Some(m) => &m[s],
                        None => model.as_ref().expect("rANS group model"),
                    })
                })
                .collect()
        };
        let default_decor = if matches!(info.channels, crate::Channels::Rgb | crate::Channels::Rgba)
            && !info.sample.is_float()
        {
            0x0f
        } else {
            1
        };
        Ok((
            Self {
                output: head,
                consumed: 0,
                info,
                cfg,
                layout,
                model,
                slot_models,
                analysis: (default_decor, 1 << crate::predict::P_MED),
                tables,
                scratch: GroupScratch::default(),
                x: 0,
                y: 0,
            },
            data_len,
        ))
    }

    /// Encoded bytes ready for the caller to transfer, possibly in small pieces.
    pub fn output(&self) -> &[u8] {
        &self.output[self.consumed..]
    }

    /// Acknowledge bytes transferred by the caller. Over-consumption fails
    /// without changing progress; acknowledging zero is allowed.
    pub fn consume_output(&mut self, count: usize) -> Result<(), Error> {
        if count > self.output().len() {
            return Err(Error::InvalidInput("group output consumption"));
        }
        self.consumed += count;
        if self.consumed == self.output.len() {
            self.output.clear();
            self.consumed = 0;
        }
        Ok(())
    }

    /// True only after every group and every pending output byte is consumed.
    pub fn is_finished(&self) -> bool {
        self.y == self.info.height && self.output().is_empty()
    }

    /// Next required rectangle. None while output remains or after completion.
    pub fn next_group(&self) -> Option<GroupRect> {
        (self.output().is_empty() && self.y < self.info.height).then(|| GroupRect {
            x: self.x,
            y: self.y,
            width: self.cfg.columns.min(self.info.width - self.x),
            height: self.cfg.rows.min(self.info.height - self.y),
        })
    }

    /// Encode exactly the next rectangle. `stride` may include row padding;
    /// the last row needs only its actual pixel bytes. Samples use native endian.
    /// Invalid input changes no progress and may be corrected and retried.
    pub fn push_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), Error> {
        let g = self.next_group().ok_or(Error::InvalidInput(
            "group encoder needs output drainage or is finished",
        ))?;
        // Restore scratch even on invalid input, preserving retry semantics.
        let mut scratch = core::mem::take(&mut self.scratch);
        let encoded = self.encode_group(g, pixels, stride, &mut scratch);
        self.scratch = scratch;
        let (body, start) = encoded?;
        self.output = body;
        self.consumed = start;
        self.advance_group(g);
        Ok(())
    }

    /// The V4 tile scheduler supplies a geometry-derived rectangle instead of
    /// consuming the V3 raster cursor. Coding remains independent per group.
    pub(crate) fn encode_rectangle(
        &mut self,
        rect: GroupRect,
        pixels: &[u8],
        stride: usize,
    ) -> Result<(Vec<u8>, usize), Error> {
        if rect.x >= self.info.width
            || rect.y >= self.info.height
            || !rect.x.is_multiple_of(self.cfg.columns)
            || !rect.y.is_multiple_of(self.cfg.rows)
            || rect.width != self.cfg.columns.min(self.info.width - rect.x)
            || rect.height != self.cfg.rows.min(self.info.height - rect.y)
        {
            return Err(Error::InvalidInput("V4 encode rectangle geometry"));
        }
        let mut scratch = core::mem::take(&mut self.scratch);
        let result = self.encode_group(rect, pixels, stride, &mut scratch);
        self.scratch = scratch;
        result
    }

    /// Nonmutating coding for rate trials in the current group row. Tables,
    /// predictor search and payload bytes are exactly those of push_group.
    #[cfg(all(feature = "std", feature = "lab"))]
    pub(crate) fn preview_group(
        &self,
        rect: GroupRect,
        pixels: &[u8],
        stride: usize,
    ) -> Result<(Vec<u8>, usize), Error> {
        let next = self
            .next_group()
            .ok_or(Error::InvalidInput("group preview needs drainage"))?;
        if rect.y != next.y
            || rect.height != next.height
            || rect.x < next.x
            || rect.x >= self.info.width
            || !rect.x.is_multiple_of(self.cfg.columns)
            || rect.width != self.cfg.columns.min(self.info.width - rect.x)
        {
            return Err(Error::InvalidInput("group preview geometry"));
        }
        self.encode_group(rect, pixels, stride, &mut GroupScratch::default())
    }

    fn encode_group(
        &self,
        g: GroupRect,
        pixels: &[u8],
        stride: usize,
        scratch: &mut GroupScratch,
    ) -> Result<(Vec<u8>, usize), Error> {
        let (w, h) = (g.width as usize, g.height as usize);
        let row = w
            .checked_mul(self.info.bytes_per_pixel())
            .ok_or(Error::TooLarge)?;
        let extent = stride
            .checked_mul(h - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::TooLarge)?;
        if stride < row || pixels.len() < extent {
            return Err(Error::InvalidInput("group input extent"));
        }
        if self.cfg.quick_pixels {
            let payload = crate::quick::unit_encode(
                pixels,
                stride,
                crate::quick::shape(&self.info, w, h),
                self.cfg.quick_bit_smarts,
                &how_far::NoPulse,
            )?;
            let mut body = Vec::new();
            body.try_reserve_exact(payload.len().checked_add(20).ok_or(Error::TooLarge)?)
                .map_err(|_| Error::TooLarge)?;
            crate::parse::records::put_record(&mut body, crate::v1::R_STRIPE, &payload);
            return Ok((body, 0));
        }
        let (decor, preds, lw) = self.analyze_into(w, h, pixels, stride, scratch);
        let widths = crate::v1::lane_widths(w, lw);
        let ns = self.layout.slot_channel.len();
        if scratch.streams.len() < ns {
            scratch.streams.resize_with(ns, Vec::new);
        }
        let active = crate::v1::active_lanes(&widths);
        let fixed = self.slot_models.is_none();
        for s in 0..ns {
            let model = self.slot_model(s);
            let table = &self.tables[if fixed { 0 } else { s }];
            let res = &scratch.residuals[s];
            for (l, symbols) in scratch.symbols.iter_mut().enumerate() {
                symbols.clear();
                rans::lane_symbols_rows(
                    res,
                    w,
                    None,
                    lw,
                    h,
                    widths[l] as usize,
                    l,
                    &model.tc,
                    4,
                    model.ctx_shift,
                    symbols,
                );
            }
            let stream = &mut scratch.streams[s];
            stream.clear();
            rans::encode_lanes_tables_scratch(
                table,
                &scratch.symbols,
                active,
                stream,
                &mut scratch.rans,
            );
        }
        // Reserve the maximum two-varint record prefix, then expose only
        // the used suffix. The Write adapter and sans-I/O path share this
        // buffer without another whole-group copy.
        const PREFIX_RESERVE: usize = 20;
        let payload_est = 1
            + preds.len().div_ceil(2)
            + scratch.streams[..ns]
                .iter()
                .map(|s| s.len() + 10)
                .sum::<usize>();
        let mut body = Vec::with_capacity(PREFIX_RESERVE + payload_est);
        body.resize(PREFIX_RESERVE, 0);
        body.push(decor);
        for pair in preds.chunks(2) {
            body.push(pair[0] | (pair.get(1).copied().unwrap_or(0) << 4));
        }
        for stream in &scratch.streams[..ns] {
            put_varint(&mut body, stream.len() as u64);
        }
        for stream in &scratch.streams[..ns] {
            body.extend_from_slice(stream);
        }
        let mut prefix = Vec::with_capacity(20);
        put_varint(&mut prefix, crate::parse::records::R_STRIPE);
        put_varint(&mut prefix, (body.len() - PREFIX_RESERVE) as u64);
        let start = PREFIX_RESERVE - prefix.len();
        body[start..PREFIX_RESERVE].copy_from_slice(&prefix);
        Ok((body, start))
    }

    /// rANS groups searched over these decorrelation and predictor masks
    /// (each group's choices are local to it) instead of mask 1 and MED.
    pub(crate) fn with_analysis(mut self, decor_mask: u8, predictor_mask: u8) -> Self {
        self.analysis = (decor_mask, predictor_mask);
        self
    }

    /// The model coding slot `s`.
    fn slot_model(&self, s: usize) -> &Model {
        match &self.slot_models {
            Some(m) => &m[s],
            None => self.model.as_ref().expect("rANS group model"),
        }
    }

    /// One rANS group's analysis into reusable `scratch`: decorrelation byte,
    /// row predictors, and lane width (residuals written to `scratch.residuals`).
    fn analyze_into(
        &self,
        w: usize,
        h: usize,
        pixels: &[u8],
        stride: usize,
        scratch: &mut GroupScratch,
    ) -> (u8, Vec<u8>, usize) {
        let nch = self.info.channels.count();
        let bps = self.info.sample.bytes();
        let sample = self.info.sample;
        let ns = self.layout.slot_channel.len();
        let px_count = w * h;
        if scratch.planes.len() < ns {
            scratch.planes.resize_with(ns, Vec::new);
        }
        if scratch.residuals.len() < ns {
            scratch.residuals.resize_with(ns, Vec::new);
        }
        for s in 0..ns {
            scratch.planes[s].resize(px_count, 0);
            scratch.residuals[s].resize(px_count, 0);
        }
        let row_bytes = w * nch * bps;
        // Fast unpack specialized by sample byte width, avoiding per-pixel
        // branch dispatch on sample size or 64-bit word splitting.
        for (s, &c) in self.layout.slot_channel.iter().enumerate() {
            let dst = &mut scratch.planes[s][..px_count];
            let ch = c % nch;
            let hi_word = c < nch;
            match bps {
                1 => {
                    if nch == 1 {
                        for y in 0..h {
                            let src = &pixels[y * stride..y * stride + w];
                            let row = &mut dst[y * w..(y + 1) * w];
                            for (o, &b) in row.iter_mut().zip(src) {
                                *o = sample.order(b as u32);
                            }
                        }
                    } else {
                        for y in 0..h {
                            let src = &pixels[y * stride..y * stride + row_bytes];
                            let row = &mut dst[y * w..(y + 1) * w];
                            for (o, px) in row.iter_mut().zip(src.chunks_exact(nch)) {
                                *o = sample.order(px[ch] as u32);
                            }
                        }
                    }
                }
                2 => {
                    let ch_off = ch * 2;
                    let step = nch * 2;
                    for y in 0..h {
                        let src = &pixels[y * stride..y * stride + row_bytes];
                        let row = &mut dst[y * w..(y + 1) * w];
                        for (o, px) in row.iter_mut().zip(src.chunks_exact(step)) {
                            let raw = u16::from_ne_bytes([px[ch_off], px[ch_off + 1]]) as u32;
                            *o = sample.order(raw);
                        }
                    }
                }
                4 => {
                    let ch_off = ch * 4;
                    let step = nch * 4;
                    for y in 0..h {
                        let src = &pixels[y * stride..y * stride + row_bytes];
                        let row = &mut dst[y * w..(y + 1) * w];
                        for (o, px) in row.iter_mut().zip(src.chunks_exact(step)) {
                            let raw = u32::from_ne_bytes([
                                px[ch_off],
                                px[ch_off + 1],
                                px[ch_off + 2],
                                px[ch_off + 3],
                            ]);
                            *o = sample.order(raw);
                        }
                    }
                }
                _ => {
                    let ch_off = ch * 8;
                    let step = nch * 8;
                    for y in 0..h {
                        let src = &pixels[y * stride..y * stride + row_bytes];
                        let row = &mut dst[y * w..(y + 1) * w];
                        for (o, px) in row.iter_mut().zip(src.chunks_exact(step)) {
                            let raw =
                                u64::from_ne_bytes(px[ch_off..ch_off + 8].try_into().unwrap());
                            *o = if hi_word {
                                sample.order((raw >> 32) as u32)
                            } else {
                                raw as u32
                            };
                        }
                    }
                }
            }
        }
        let refs: Vec<&[u32]> = scratch.planes[..ns]
            .iter()
            .map(|p| &p[..px_count])
            .collect();
        let mut residual_refs: Vec<&mut [u32]> = scratch.residuals[..ns]
            .iter_mut()
            .map(|r| &mut r[..px_count])
            .collect();
        let lw = w.div_ceil(16);
        let cfg = AnalysisConfig {
            decor_mask: self.analysis.0,
            predictor_mask: self.analysis.1,
            threads: 1,
            ..Default::default()
        };
        let (decor, preds) = if self.layout.is_near() {
            crate::near::analyze_stripe_near(
                &self.layout,
                &refs,
                w,
                lw,
                0,
                h,
                &cfg,
                &mut residual_refs,
            )
        } else {
            crate::encode::analyze_stripe_fast(
                &self.layout,
                &refs,
                w,
                lw,
                0,
                h,
                &cfg,
                &mut residual_refs,
                None,
            )
        };
        (decor, preds, lw)
    }

    /// Adds a `w` x `h` rANS group's token counts per slot and context
    /// class (`hists[slot][class][token]`, as the lossless encoder's pass 1
    /// counts them) without coding it: what measured models are built from.
    pub(crate) fn group_histograms(
        &self,
        w: usize,
        h: usize,
        pixels: &[u8],
        stride: usize,
        hists: &mut [Vec<Vec<u64>>],
    ) {
        let mut scratch = GroupScratch::default();
        let (_, _, lw) = self.analyze_into(w, h, pixels, stride, &mut scratch);
        let widths = crate::v1::lane_widths(w, lw);
        let tc = &self.model.as_ref().expect("rANS group model").tc;
        for (res, hist) in scratch.residuals.iter().zip(hists.iter_mut()) {
            for l in 0..NL {
                rans::lane_hist_rows(res, w, None, lw, h, widths[l] as usize, l, tc, 4, 0, hist);
            }
        }
    }

    /// Empty per-slot class histograms for [`Self::group_histograms`].
    pub(crate) fn empty_histograms(&self) -> Vec<Vec<Vec<u64>>> {
        let asz = self.model.as_ref().expect("rANS group model").tc.alphabet() as usize;
        vec![
            vec![vec![0u64; asz]; crate::coding_layout::CTX_CLASSES];
            self.layout.slot_channel.len()
        ]
    }

    /// [`Self::nested`] with each slot coded by a model built from `hists`
    /// (context classes grouped as the lossless encoder groups them, one
    /// table per context).
    pub(crate) fn nested_measured(
        info: ImageInfo,
        cfg: GroupEncodeConfig,
        hists: &[Vec<Vec<u64>>],
    ) -> Result<(Self, Vec<u8>), Error> {
        let tc = TokenCfg::v1(4, 1, info.sample.word_depth());
        let models = hists.iter().map(|hist| measured_model(tc, hist)).collect();
        let (mut session, data_len) = Self::new_rows(
            info,
            &Metadata::default(),
            cfg,
            false,
            cfg.rows,
            Some(models),
        )?;
        let at = session.output.len() - data_len as usize;
        let head = session.output[at..].to_vec();
        session.consumed = session.output.len();
        Ok((session, head))
    }

    fn advance_group(&mut self, g: GroupRect) {
        self.x += g.width;
        if self.x == self.info.width {
            self.x = 0;
            self.y += g.height;
        }
    }

    /// Require every group and all output to have been consumed.
    pub fn finish(self) -> Result<(), Error> {
        if !self.is_finished() {
            return Err(Error::InvalidInput("incomplete group image"));
        }
        Ok(())
    }
}

/// `std::io::Write` adapter for [`GroupEncodeSession`]. A sink failure is
/// sticky because partially written records cannot be replayed safely.
#[cfg(feature = "std")]
pub struct GroupEncoder<W> {
    writer: W,
    session: GroupEncodeSession,
    failed: bool,
}

#[cfg(feature = "std")]
impl<W: Write> GroupEncoder<W> {
    /// Admit the image before writing its header.
    pub fn new(
        writer: W,
        info: ImageInfo,
        meta: &Metadata,
        cfg: &crate::ImiEncoderConfig,
    ) -> Result<Self, StreamEncodeError> {
        let session = GroupEncodeSession::new(info, meta, cfg)?;
        let mut encoder = Self {
            writer,
            session,
            failed: false,
        };
        encoder.drain()?;
        Ok(encoder)
    }

    fn drain(&mut self) -> Result<(), StreamEncodeError> {
        self.failed = true;
        let count = self.session.output().len();
        self.writer.write_all(self.session.output())?;
        self.session.consume_output(count)?;
        self.failed = false;
        Ok(())
    }

    /// The next input rectangle, or None after completion or a sink failure.
    pub fn next_group(&self) -> Option<GroupRect> {
        if self.failed {
            None
        } else {
            self.session.next_group()
        }
    }

    /// Encode a strided rectangle. Invalid input may be corrected and retried;
    /// a sink error is sticky and partial output must be discarded.
    pub fn write_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), StreamEncodeError> {
        if self.failed {
            return Err(Error::InvalidInput("group encoder is finished or failed").into());
        }
        self.session.push_group(pixels, stride)?;
        self.drain()
    }

    /// Require every group, flush and return the caller-owned writer.
    pub fn finish(mut self) -> Result<W, StreamEncodeError> {
        if self.failed {
            return Err(Error::InvalidInput("incomplete group image").into());
        }
        self.session.finish()?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

/// Calibrated 4-context full-support fixed prior for one-pass bounded groups.
///
/// Uses Lomax (heavy-tailed Pareto Type II) value priors + power-law zero-run
/// priors across four neighbor-activity contexts (flat/run, smooth, medium,
/// rough/noisy), keeping positive weight on every legal symbol below the run
/// cap so any future group encodes without a statistics pass. Depends only on
/// `(info.sample)` (never image geometry) so growing/append heads stay canonical.
fn fixed_group_model(info: &ImageInfo) -> Result<Model, Error> {
    let depth = info.sample.word_depth();
    let tc = TokenCfg::v1(4, 1, depth);
    let asz = tc.alphabet() as usize;
    let vt = tc.value_tokens() as usize;
    let legal_end = rans::run_cap_end(&tc).min(asz);
    let is_wide_float = depth > 16 && info.sample.is_float();
    let ctx_shift = if is_wide_float { 10u32 } else { 0u32 };
    let base_scale = (1u64 << ctx_shift) as f64;
    let ctx_map: [u8; rans::CLASSES] = [0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3];
    // (scale, power, zero_mass, run_mass) per context.
    let s3 = if depth <= 8 {
        64.0
    } else if depth <= 16 {
        256.0
    } else if is_wide_float {
        131_072.0
    } else {
        2048.0
    };
    let p3 = if depth <= 8 {
        1.2
    } else if depth <= 16 {
        1.0
    } else if is_wide_float {
        0.55
    } else {
        0.85
    };
    let params: [(f64, f64, f64, f64, f64); NCTX] = if is_wide_float {
        // Wide floats frequently jump across exponents (0.0 <-> 1e-6 or
        // NaN <-> finite) straight out of flat or rough regions; keep a small
        // log-uniform floor on contexts 0 and 3 while keeping contexts 1-2 sharp.
        [
            (3.0 * base_scale, 1.0, 0.34, 0.34, 0.10),
            (3.5 * base_scale, 1.5, 0.15, 0.04, 0.01),
            (16.0 * base_scale, 1.4, 0.05, 0.015, 0.01),
            (s3, p3, 0.015, 0.01, 0.08),
        ]
    } else {
        [
            (2.0 * base_scale, 1.4, 0.36, 0.34, 0.0),
            (3.5 * base_scale, 1.6, 0.16, 0.04, 0.0),
            (16.0 * base_scale, 1.5, 0.05, 0.015, 0.0),
            (s3, p3, 0.015, 0.01, 0.0),
        ]
    };
    let mut freq = Vec::with_capacity(NCTX);
    let mut weights = Vec::with_capacity(NCTX);
    let prec = vec![4u32; NCTX];
    for &(scale, power, zero_mass, run_mass, floor_share) in &params {
        let mut probs = vec![0.0f64; asz];
        let val_mass = 1.0 - zero_mass - run_mass;
        probs[0] = zero_mass;
        let mut val_sum = 0.0f64;
        for (tok, p) in probs.iter_mut().enumerate().take(vt).skip(1) {
            let (base, nb) = crate::tokens::hybrid_base(tok as u32, tc.k, tc.m);
            let lo = base as f64;
            let hi1 = lo + (1u64 << nb) as f64;
            let m = libm::pow(1.0 + lo / scale, -power) - libm::pow(1.0 + hi1 / scale, -power);
            *p = m.max(0.0);
            val_sum += *p;
        }
        if val_sum > 0.0 {
            let k = (val_mass * (1.0 - floor_share)) / val_sum;
            let floor = (val_mass * floor_share) / (vt.saturating_sub(1).max(1) as f64);
            for p in &mut probs[1..vt] {
                *p = *p * k + floor;
            }
        }
        let mut run_sum = 0.0f64;
        for (tok, p) in probs.iter_mut().enumerate().take(legal_end).skip(vt) {
            let (base, _nb) = crate::tokens::hybrid_base((tok - vt) as u32, tc.rk, tc.rm);
            let len = base + 1;
            if len >= 4 {
                let m = libm::pow(len as f64, -0.45);
                *p = m;
                run_sum += m;
            }
        }
        if run_sum > 0.0 {
            let k = run_mass / run_sum;
            for p in &mut probs[vt..legal_end] {
                *p *= k;
            }
        }
        let mut wt = vec![0u32; asz];
        for tok in 0..legal_end {
            let raw = (probs[tok] * 65536.0) as u64;
            wt[tok] = rans::quantize_weight(raw.max(1), 4).max(1);
        }
        rans::enforce_run_cap(&tc, &mut wt);
        let f = rans::weights_to_freq(&wt).ok_or(Error::TooLarge)?;
        weights.push(wt);
        freq.push(f);
    }
    Ok(Model {
        tc,
        ctx_map,
        freq,
        weights,
        prec,
        cross: false,
        ctx_shift,
        seeds: [None; NCTX],
    })
}

/// A slot model from class histograms, as the lossless encoder builds one
/// without seed tables: contexts by `choose_ctx_map`, one table each.
fn measured_model(tc: TokenCfg, hist: &[Vec<u64>]) -> Model {
    let asz = tc.alphabet() as usize;
    let (map18, groups) = crate::encode::choose_ctx_map(hist, rans::NCTX, 12);
    let mut ctx_map = [0u8; rans::CLASSES];
    ctx_map.copy_from_slice(&map18[..rans::CLASSES]);
    let mut freq = vec![vec![0u32; asz]; NCTX];
    let mut weights = vec![vec![0u32; asz]; NCTX];
    let mut prec = vec![1u32; NCTX];
    for c in 0..NCTX {
        match groups.get(c) {
            Some(g) if g.iter().any(|&v| v != 0) => {
                let (mut wt, mut f, p) = rans::choose_table(g);
                rans::enforce_run_cap(&tc, &mut wt);
                if let Some(f2) = rans::weights_to_freq(&wt) {
                    f = f2;
                }
                freq[c] = f;
                weights[c] = wt;
                prec[c] = p;
            }
            _ => {
                freq[c][0] = rans::M;
                weights[c][0] = 1;
            }
        }
    }
    Model {
        tc,
        ctx_map,
        freq,
        weights,
        prec,
        cross: false,
        ctx_shift: 0,
        seeds: [None; NCTX],
    }
}
