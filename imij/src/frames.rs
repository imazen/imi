//! Multiple frames (animation) and pages (SPEC.md 1.3).
//!
//! `DATA` holds frame 0 / page 1, so decoders that do not know frames show
//! it. An `aNIM` chunk before `DATA` says what the file holds; every further
//! frame or page is an `fRAM` chunk: a small frame header and a complete
//! nested IMI file with the frame's pixels (its own size and coding).
//!
//! Animation frames are composited on a canvas the size of the file's
//! image: a frame covers a rectangle of the canvas; an intra frame covers
//! all of it, a frame that uses the previous frame keeps the previous
//! pixels outside its rectangle (and, with inter coding, inside it where it
//! says so). Decoding writes into the buffer holding the previous frame, so
//! a decoder needs no memory beyond the canvas itself.
//!
//! Pages are independent. Uniform pages (`aNIM` kind 2) share the first
//! page's pixel format, color and metadata; mixed pages (kind 3) are each
//! described by their own nested file, so one document can hold bilevel,
//! gray and color pages, lossless or lossy.

mod admitted;
pub(crate) mod defaults;
mod metadata_probe;
#[cfg(all(test, feature = "encode"))]
mod progress_tests;
#[cfg(all(test, feature = "std", feature = "encode"))]
mod row_tests;
#[cfg(feature = "encode")]
mod sequence;
mod storage;
#[cfg(all(test, feature = "std", feature = "lab", feature = "encode"))]
pub(crate) mod v4;
#[cfg(feature = "encode")]
use crate::config::Lowered;
use alloc::borrow::Cow;
use core::ops::Range;
#[cfg(feature = "encode")]
use how_far::{Pulse, Stop};
pub use metadata_probe::{
    IndexedAttribute, IndexedMetadataQuery, IndexedMetadataSummary, probe_indexed_nested_summary,
};
#[cfg(feature = "encode")]
pub use sequence::{ImageReference, SequenceEncoder, SequenceItem, SequenceKind};
use storage::{ImageRecord, PixelTable, span};

#[cfg(feature = "encode")]
use alloc::vec;
use alloc::vec::Vec;

use crate::container;
#[cfg(feature = "encode")]
use crate::parse::chunks::ANIM;
use crate::parse::chunks::FRAM;
use crate::parse::header::{Feature, Features};
use crate::parse::nested::NestedPolicy;
use crate::parse::records::head_records;
#[cfg(feature = "encode")]
use crate::put_varint;
use crate::{ByteReader, Coding, Error, ImageInfo, Metadata};

/// An exact frame duration: `num / den` seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameDuration {
    pub num: u64,
    pub den: u32,
}

impl FrameDuration {
    /// No duration (pages, or a frame shown for no time).
    pub const ZERO: FrameDuration = FrameDuration { num: 0, den: 1 };

    pub const fn from_millis(ms: u32) -> FrameDuration {
        FrameDuration {
            num: ms as u64,
            den: 1000,
        }
    }
}

/// What a file holds. The `aNIM` kind is open (SPEC 0.3), so the enum is
/// `#[non_exhaustive]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameKind {
    /// One image (no `aNIM` chunk).
    Still,
    /// Frames composited on a canvas, with durations and a loop count.
    Animation,
    /// Independent pages, each with its own size; every page has the first
    /// page's pixel format, color and metadata.
    Pages,
    /// Independent pages, each with its own size, pixel format (channels,
    /// sample type, significant bits), color, metadata and coding
    /// (`aNIM` kind 3). Readers that predate it reject it with
    /// [`Error::Unsupported`] through the header's feature `mixed_pages`.
    MixedPages,
}

/// Encoder settings for animations and pages.
///
/// New fields can arrive in any release: start from [`FrameConfig::new`] and
/// assign the fields to change.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[cfg(feature = "encode")]
pub struct FrameConfig {
    /// The encoder for every frame or page.
    pub coding: crate::ImiEncoderConfig,
    /// Animation loop count (0 = forever).
    pub loop_count: u32,
    /// Code animation frames against the previous frame (only the changed
    /// rectangle, and inside it what changed) when that is smaller.
    pub inter: bool,
    /// Animation: an intra frame (a seek point) at least every this many
    /// frames; 0 = only when it is smaller.
    pub keyframe_interval: u32,
}

#[cfg(feature = "encode")]
impl FrameConfig {
    pub fn new(coding: crate::ImiEncoderConfig) -> FrameConfig {
        FrameConfig {
            coding,
            loop_count: 0,
            inter: true,
            keyframe_interval: 0,
        }
    }
}

/// Decodes a frame file the encoder just wrote into `out`, the canvas a
/// decoder holds next: `inter`: `out` holds the previous frame; `canvas`:
/// the file whose color a frame inherits.
#[cfg(feature = "encode")]
fn decode_file(
    file: &[u8],
    out: &mut [u8],
    stride: usize,
    threads: usize,
    inter: bool,
    canvas: Option<&ImageInfo>,
) -> Result<(), Error> {
    let policy = NestedPolicy {
        verify: false,
        limits: None,
    };
    let f = policy.parse_decodable(file)?;
    crate::v1::decode_parsed_frame(&f, out, stride, threads, inter, canvas).map(|_| ())
}

/// The feature set of a nested file (a frame or a page): its mask and what
/// its own fields imply. The outer header's set covers them all (SPEC 0.1,
/// 1.3).
#[cfg(feature = "encode")]
fn declared(file: &[u8]) -> Result<Features, Error> {
    Ok(crate::parse::header::read_versioned(&mut ByteReader::new(file), u64::MAX)?.declared)
}

/// `fRAM` presence bits (SPEC 1.3): the frame uses the previous one, the
/// frame is empty, a frame record list follows.
const F_PREVIOUS: u64 = 1;
const F_EMPTY: u64 = 2;
const F_RECORDS: u64 = 4;

/// A mixed document has a page this build does not decode.
const MIXED_UNDECODABLE: &str = "mixed pages need a profile this build does not decode";
/// A frame's or page's feature set is not covered by the outer header's.
const FRAME_UNDECLARED: &str = "frame needs a feature the file does not declare";
const PAGE_UNDECLARED: &str = "page needs a feature the file does not declare";
/// Header of one frame or page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub index: usize,
    /// Rectangle on the canvas (animation); pages: (0, 0, page size).
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub duration: FrameDuration,
    /// The frame keeps the previous frame's pixels outside its rectangle
    /// (decode into the buffer that holds the previous frame). For unified
    /// sequences this is true only for an immediate predecessor; consult
    /// [`FrameDecoder::reference_index`] to distinguish other dependencies
    /// from independent keyframes.
    pub uses_previous: bool,
    /// The frame's coding; `None` for a frame that repeats the previous one.
    pub coding: Option<Coding>,
}

/// A previously reconstructed unified-sequence item used to decode one
/// indexed nested file. Its digest is the reference-raster digest declared by
/// that item's frame control, not a compressed-byte checksum.
pub struct IndexedReference<'a> {
    pub index: u64,
    /// The reference-raster digest as the frame record declares it
    /// ([`FrameSeekInfo::reference_hash`]).
    pub hash: crate::Digest,
    pub info: &'a ImageInfo,
    pub pixels: &'a [u8],
    pub stride: usize,
}

/// Read an indexed item's image declaration and metadata through its DATA
/// header. The prefix can end immediately after that header. The caller
/// binds the prefix to the outer file and its index, as for indexed decode.
pub fn probe_indexed_nested(
    nested_head: &[u8],
    outer: &crate::Header,
) -> Result<(ImageInfo, Metadata), Error> {
    use crate::decode_resources::{DecodePolicy, DecodeSession};
    use crate::parse::chunks::{self, Kind, Rules};
    use crate::parse::limits::Admission;

    let session = DecodeSession::new(DecodePolicy::default());
    // Bound metadata and headers without admitting a full output raster.
    let admission = session.v4_models();
    let mut input = ByteReader::new(nested_head);
    let parsed =
        crate::parse::header::read_versioned(&mut input, session.v4_resume().header_limit())?;
    admission.header(&parsed.header.info, parsed.len)?;
    if !outer.features.covers(parsed.declared) {
        return Err(Error::Corrupt(FRAME_UNDECLARED));
    }
    let mut metadata = Metadata {
        orientation: parsed.header.orientation,
        ..Metadata::default()
    };
    let mut rules = Rules::new(parsed.mixed_pages(), false)
        .with_sequence_items(parsed.declared.contains(Feature::SequenceItems));
    loop {
        let (tag, len) = chunks::read_head(&mut input)?;
        admission.chunk_start()?;
        match rules.head(tag, len)? {
            Kind::Data => return Ok((parsed.header.info, metadata)),
            Kind::Anim | Kind::Frame => return Err(Error::Corrupt("sequence item holds frames")),
            kind => {
                let body = chunks::payload(
                    &mut input,
                    len.ok_or(Error::Corrupt("open nested metadata"))?,
                )?;
                if kind.has_body_rules(&tag) {
                    rules.identity(tag, body)?;
                }
                admission.chunk(tag, body)?;
                chunks::record(tag, body)?;
                if kind == Kind::Meta {
                    container::keep_metadata(&mut metadata, tag, body)?;
                }
            }
        }
    }
}

/// Decode one complete nested file located by an external sequence index.
///
/// This applies the ordinary nested-file parser and reconstruction path, but
/// cannot validate the enclosing file's `fRAM` controls or whole-file checksum.
/// The caller must bind the index to an immutable outer file, verify its
/// location/control record, and enforce any outer decoded-pixel checksum.
/// Use [`FrameDecoder`] when complete-file admission is required.
pub fn decode_indexed_nested(
    file: &[u8],
    outer: &crate::Header,
    reference: Option<IndexedReference<'_>>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
) -> Result<ImageInfo, Error> {
    let nested = parse_indexed_nested(file, outer)?;
    reconstruct_indexed_nested(&nested, reference, out, stride, threads)?;
    Ok(nested.header.info)
}

/// Decode one indexed item and return its own metadata. A crop uses the
/// region reader for independent items. Referenced items require full
/// reconstruction with the supplied reference.
pub fn decode_indexed_nested_with_metadata(
    file: &[u8],
    outer: &crate::Header,
    reference: Option<IndexedReference<'_>>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
    roi: Option<crate::planes::Rect>,
) -> Result<(ImageInfo, Metadata), Error> {
    if roi.is_some() && reference.is_some() {
        return Err(Error::Unsupported("ROI of referenced indexed item"));
    }
    let nested = parse_indexed_nested(file, outer)?;
    let metadata = nested.metadata()?;
    let mut info = nested.header.info.clone();
    if let Some(rect) = roi {
        let mut reader = crate::region::RegionReader::open(crate::planes::SliceReader::new(file))?;
        reader.decode_region_threads(rect, out, stride, threads)?;
        info.width = rect.width();
        info.height = rect.height();
    } else {
        reconstruct_indexed_nested(&nested, reference, out, stride, threads)?;
    }
    Ok((info, metadata))
}

fn parse_indexed_nested<'a>(
    file: &'a [u8],
    outer: &crate::Header,
) -> Result<container::File<'a>, Error> {
    let policy = NestedPolicy {
        verify: true,
        limits: None,
    };
    let nested = policy.parse_decodable(file)?;
    if nested.anim.is_some() || !nested.frames.is_empty() {
        return Err(Error::Corrupt("sequence item holds frames"));
    }
    if !outer.features.covers(nested.declared) {
        return Err(Error::Corrupt(FRAME_UNDECLARED));
    }
    Ok(nested)
}

fn reconstruct_indexed_nested(
    nested: &container::File<'_>,
    reference: Option<IndexedReference<'_>>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
) -> Result<(), Error> {
    let info = &nested.header.info;
    if let Some(reference) = &reference {
        if reference.info != info {
            return Err(Error::Corrupt("reference pixel description differs"));
        }
        let row = info.row_bytes();
        let needed = info
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("invalid image layout"))?;
        if out.len() < needed {
            return Err(Error::InvalidInput("output buffer too small"));
        }
        if crate::pixelsum::digest_with(
            reference.hash.algorithm(),
            info,
            reference.index,
            reference.pixels,
            reference.stride,
        )? != reference.hash
        {
            return Err(Error::Corrupt("reference raster checksum mismatch"));
        }
        for y in 0..info.height as usize {
            out[y * stride..y * stride + row].copy_from_slice(
                &reference.pixels[y * reference.stride..y * reference.stride + row],
            );
        }
    }
    crate::v1::decode_parsed_frame(nested, out, stride, threads, reference.is_some(), None)?;
    Ok(())
}

/// Check an indexed unified item's on-wire `fRAM` header and controls before
/// decoding the nested file immediately after `prefix`.
///
/// The prefix starts at the `fRAM` tag and ends immediately before its nested
/// file; the index supplies that file's length and declared reference. This
/// bounded check prevents an index from silently replacing changed controls.
///
/// `version` is the outer file's header version ([`crate::Header::version`]),
/// which numbers the frame records.
pub fn check_indexed_frame_prefix(
    prefix: &[u8],
    version: u8,
    nested_len: u64,
    ordinal: u64,
    reference: Option<(u64, crate::Digest)>,
) -> Result<(), Error> {
    crate::parse::header::check_header_version(version)?;
    let mut reader = ByteReader::new(prefix);
    let (tag, len) = crate::parse::chunks::read_head(&mut reader)?;
    if tag != FRAM {
        return Err(Error::Unsupported("indexed item is not adjacent to fRAM"));
    }
    let controls_len = (prefix.len() - reader.pos) as u64;
    if len != controls_len.checked_add(nested_len) {
        return Err(Error::Corrupt("indexed fRAM length differs"));
    }
    let controls = frame_control(&prefix[reader.pos..], version)?;
    if !controls.nested.is_empty()
        || controls.x0 != 0
        || controls.y0 != 0
        || controls.duration.den == 0
        || controls.present & (F_PREVIOUS | F_EMPTY) != 0
        || controls.reference != reference
        || controls
            .reference
            .is_some_and(|(index, _)| index >= ordinal)
    {
        return Err(Error::Corrupt("indexed fRAM controls differ"));
    }
    Ok(())
}

#[cfg(feature = "encode")]
fn encode_image(
    coding: &Lowered,
    px: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
) -> Result<Vec<u8>, Error> {
    encode_image_checked(coding, px, stride, info, meta, &how_far::NoPulse)
}

#[cfg(feature = "encode")]
fn encode_image_checked(
    coding: &Lowered,
    px: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    // Frame 0 becomes the outer file and later frames nest in it: neither
    // carries a checksum of its own ([`FrameEncoder::finish`] appends one).
    match coding {
        Lowered::Quick(c) => crate::quick::encode_frame(px, stride, info, meta, c, pulse),
        Lowered::Lossless(c) => {
            crate::v1::encode_frame_with_progress(px, stride, info, meta, c, None, pulse)
        }
        Lowered::Lossy(c) => {
            crate::lossy::encode_lossy_frame_with_progress(px, stride, info, meta, c, None, pulse)
        }
    }
}

/// Encodes an animation or a document of pages.
#[cfg(feature = "encode")]
pub struct FrameEncoder {
    canvas: ImageInfo,
    meta: Metadata,
    kind: FrameKind,
    cfg: FrameConfig,
    first: Option<(Vec<u8>, FrameDuration)>,
    frames: Vec<Vec<u8>>,
    /// Animation: the previous frame's source pixels (tight rows), retained
    /// only when inter coding is enabled.
    prev_src: Vec<u8>,
    /// Frames since the last intra frame.
    since_key: u32,
    /// The features the frames or pages after the first declare: the outer
    /// header declares them too (SPEC 0.1, 1.3).
    later_features: Features,
}

#[cfg(feature = "encode")]
impl FrameEncoder {
    /// An animation on a canvas of `canvas` size and pixel format. `meta`
    /// (ICC, EXIF, ...) applies to every frame.
    pub fn animation(canvas: ImageInfo, meta: Metadata, cfg: FrameConfig) -> FrameEncoder {
        FrameEncoder {
            canvas,
            meta,
            kind: FrameKind::Animation,
            cfg,
            first: None,
            frames: Vec::new(),
            prev_src: Vec::new(),
            since_key: 0,
            later_features: Features::EMPTY,
        }
    }

    /// Pages with the channels and sample type of `info` (each page has its
    /// own size).
    pub fn pages(info: ImageInfo, meta: Metadata, cfg: FrameConfig) -> FrameEncoder {
        FrameEncoder {
            canvas: info,
            meta,
            kind: FrameKind::Pages,
            cfg,
            first: None,
            frames: Vec::new(),
            prev_src: Vec::new(),
            since_key: 0,
            later_features: Features::EMPTY,
        }
    }

    /// Pages that each carry their own pixel format, color, metadata and
    /// coding (see [`Self::push_page_with`]). Readers that predate mixed
    /// pages reject such a file's pages with [`Error::Unsupported`]; use
    /// [`Self::pages`] when every page shares one pixel format and color.
    pub fn mixed_pages(cfg: FrameConfig) -> FrameEncoder {
        FrameEncoder {
            canvas: ImageInfo::new(1, 1, crate::Channels::Gray, crate::SampleType::U8),
            meta: Metadata::default(),
            kind: FrameKind::MixedPages,
            cfg,
            first: None,
            frames: Vec::new(),
            prev_src: Vec::new(),
            since_key: 0,
            later_features: Features::EMPTY,
        }
    }

    /// Adds a full-canvas animation frame.
    pub fn push_frame(
        &mut self,
        pixels: &[u8],
        stride: usize,
        duration: FrameDuration,
    ) -> Result<(), Error> {
        self.push_frame_checked(pixels, stride, duration, &how_far::NoPulse)
    }

    pub(crate) fn push_frame_checked(
        &mut self,
        pixels: &[u8],
        stride: usize,
        duration: FrameDuration,
        pulse: &dyn Pulse,
    ) -> Result<(), Error> {
        pulse.check().map_err(|_| Error::Cancelled)?;
        if self.kind != FrameKind::Animation {
            return Err(Error::InvalidInput("not an animation encoder"));
        }
        if duration.den == 0 {
            return Err(Error::InvalidInput("zero duration denominator"));
        }
        let info = self.canvas.clone();
        let extent = info
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("invalid image layout"))?;
        if pixels.len() < extent {
            return Err(Error::InvalidInput("pixel buffer too small"));
        }
        let rb = info.row_bytes();
        let w = info.width as usize;
        let h = info.height as usize;
        let coding = self.cfg.coding.lower();
        if self.first.is_none() {
            let f = encode_image_checked(&coding, pixels, stride, &info, &self.meta, pulse)?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            if self.cfg.inter {
                self.store_prev_src(pixels, stride, rb, h)?;
            }
            self.first = Some((f, duration));
            return Ok(());
        }
        if !self.cfg.inter {
            let f =
                encode_image_checked(&coding, pixels, stride, &info, &Metadata::default(), pulse)?;
            pulse.check().map_err(|_| Error::Cancelled)?;
            let payload = frame_payload(0, 0, duration, 0, &f);
            self.frames.try_reserve(1).map_err(|_| Error::TooLarge)?;
            self.frames.push(payload);
            self.since_key = 0;
            return Ok(());
        }
        let bpp = rb / w;
        let key_due = self.cfg.keyframe_interval != 0
            && self.since_key.saturating_add(1) >= self.cfg.keyframe_interval;
        let changed = changed_rect(&self.prev_src, rb, pixels, stride, w, h, bpp, pulse)?;
        if changed.is_none() && !key_due {
            let payload = frame_payload(0, 0, duration, F_PREVIOUS | F_EMPTY, &[]);
            self.frames.try_reserve(1).map_err(|_| Error::TooLarge)?;
            self.frames.push(payload);
            self.since_key = self.since_key.saturating_add(1);
            return Ok(());
        }
        // An unchanged source still needs an independent image at a seek point.
        let (x0, y0, x1, y1) = changed.unwrap_or((0, 0, w, h));
        let (rw, rh) = (x1 - x0, y1 - y0);
        let area = rw * rh;
        let full_canvas = x0 == 0 && y0 == 0 && x1 == w && y1 == h;
        let mut best: Option<(Vec<u8>, bool)> = None;
        // Near-lossless frames never predict from the previous frame (SPEC
        // 2.6): code them intra. Coders without pixel-level reference
        // prediction gain nothing over an independent intra frame when the
        // changed rectangle already covers the entire canvas.
        if !key_due && !self.near() && (!full_canvas || uses_pixel_reference(&coding, &info)) {
            let ri = ImageInfo {
                width: rw as u32,
                height: rh as u32,
                ..info.clone()
            };
            let cur = &pixels[y0 * stride + x0 * bpp..];
            let refr = &self.prev_src[y0 * rb + x0 * bpp..];
            best = Some((
                encode_inter(&coding, cur, stride, &ri, refr, rb, pulse)?,
                true,
            ));
        }
        if best.is_none() || key_due || 2 * area >= w * h {
            let f =
                encode_image_checked(&coding, pixels, stride, &info, &Metadata::default(), pulse)?;
            if best.as_ref().is_none_or(|(b, _)| f.len() < b.len()) {
                best = Some((f, false));
            }
        }
        pulse.check().map_err(|_| Error::Cancelled)?;
        let (f, inter) = best.unwrap();
        let payload = if inter {
            frame_payload(x0 as u32, y0 as u32, duration, F_PREVIOUS, &f)
        } else {
            frame_payload(0, 0, duration, 0, &f)
        };
        self.frames.try_reserve(1).map_err(|_| Error::TooLarge)?;
        if let Some((cx0, cy0, cx1, cy1)) = changed {
            self.update_prev_src_rect(pixels, stride, rb, bpp, cx0, cy0, cx1, cy1);
        }
        self.frames.push(payload);
        if inter {
            self.since_key = self.since_key.saturating_add(1);
        } else {
            self.since_key = 0;
        }
        Ok(())
    }

    /// Copies the first frame into the tight-packed reference buffer.
    fn store_prev_src(
        &mut self,
        pixels: &[u8],
        stride: usize,
        rb: usize,
        h: usize,
    ) -> Result<(), Error> {
        let total = rb.checked_mul(h).ok_or(Error::TooLarge)?;
        self.prev_src.clear();
        self.prev_src
            .try_reserve_exact(total)
            .map_err(|_| Error::TooLarge)?;
        if stride == rb {
            self.prev_src.extend_from_slice(&pixels[..total]);
        } else {
            for y in 0..h {
                self.prev_src
                    .extend_from_slice(&pixels[y * stride..y * stride + rb]);
            }
        }
        Ok(())
    }

    /// Updates the changed rectangle of `prev_src` in place, reusing its allocation.
    #[allow(clippy::too_many_arguments)]
    fn update_prev_src_rect(
        &mut self,
        pixels: &[u8],
        stride: usize,
        rb: usize,
        bpp: usize,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
    ) {
        if stride == rb && x0 == 0 && x1 * bpp == rb {
            self.prev_src[y0 * rb..y1 * rb].copy_from_slice(&pixels[y0 * rb..y1 * rb]);
            return;
        }
        let x_off = x0 * bpp;
        let row_len = (x1 - x0) * bpp;
        for y in y0..y1 {
            self.prev_src[y * rb + x_off..y * rb + x_off + row_len]
                .copy_from_slice(&pixels[y * stride + x_off..y * stride + x_off + row_len]);
        }
    }

    /// Lossless frames coded near-lossless (any channel bound).
    fn near(&self) -> bool {
        let n = self.canvas.channels.count();
        matches!(self.cfg.coding.lower(), Lowered::Lossless(c) if c.max_error[..n].iter().any(|&d| d != 0))
    }

    /// Adds a page (`info`: its size; channels and sample type as the
    /// encoder's). For mixed pages, a page in `info`'s own format and color
    /// with no metadata, coded with the encoder's coding.
    pub fn push_page(
        &mut self,
        pixels: &[u8],
        stride: usize,
        info: &ImageInfo,
    ) -> Result<(), Error> {
        if self.kind == FrameKind::MixedPages {
            let coding = self.cfg.coding.clone();
            return self.push_page_with(pixels, stride, info, &Metadata::default(), &coding);
        }
        if self.kind != FrameKind::Pages {
            return Err(Error::InvalidInput("not a pages encoder"));
        }
        if info.channels != self.canvas.channels
            || info.alpha_mode != self.canvas.alpha_mode
            || info.sample != self.canvas.sample
            || (0..info.channels.count())
                .any(|c| info.channel_bits(c) != self.canvas.channel_bits(c))
        {
            return Err(Error::InvalidInput("page pixel format differs"));
        }
        let mut info = info.clone();
        info.cicp = self.canvas.cicp;
        if self.first.is_none() {
            let f = encode_image(&self.cfg.coding.lower(), pixels, stride, &info, &self.meta)?;
            self.canvas = info;
            self.first = Some((f, FrameDuration::ZERO));
            return Ok(());
        }
        let f = encode_image(
            &self.cfg.coding.lower(),
            pixels,
            stride,
            &info,
            &Metadata::default(),
        )?;
        self.frames
            .push(frame_payload(0, 0, FrameDuration::ZERO, 0, &f));
        Ok(())
    }

    /// Adds a page of a mixed-pages encoder ([`Self::mixed_pages`]) with its
    /// own pixel format, size and color (`info`), metadata (`meta`: ICC,
    /// EXIF, XMP, orientation, ...) and coding. The first page's `info` and
    /// `meta` are the file's header and metadata, which readers that know no
    /// pages show.
    pub fn push_page_with(
        &mut self,
        pixels: &[u8],
        stride: usize,
        info: &ImageInfo,
        meta: &Metadata,
        coding: &crate::ImiEncoderConfig,
    ) -> Result<(), Error> {
        if self.kind != FrameKind::MixedPages {
            return Err(Error::InvalidInput("not a mixed-pages encoder"));
        }
        let f = encode_image(&coding.lower(), pixels, stride, info, meta)?;
        if self.first.is_none() {
            self.canvas = info.clone();
            self.meta = meta.clone();
            self.first = Some((f, FrameDuration::ZERO));
            return Ok(());
        }
        self.later_features = self.later_features.union(declared(&f)?);
        self.frames
            .push(frame_payload(0, 0, FrameDuration::ZERO, 0, &f));
        Ok(())
    }

    /// The finished file, with a whole-file checksum when the frame coding's
    /// configuration asks for one (nested frames and pages have none).
    pub fn finish(self) -> Result<Vec<u8>, Error> {
        self.finish_checked(&how_far::NoPulse)
    }

    /// Finish with the retained V3 collection grammar. Use this for compatibility
    /// fixtures and consumers that explicitly require V3; [`Self::finish`] emits V4.
    pub fn finish_legacy(self) -> Result<Vec<u8>, Error> {
        self.finish_native_checked(&how_far::NoPulse)
    }

    pub(crate) fn finish_checked(mut self, stop: &dyn Stop) -> Result<Vec<u8>, Error> {
        stop.check().map_err(|_| Error::Cancelled)?;
        self.prev_src = Vec::new();
        let (first, d0) = self.first.take().ok_or(Error::InvalidInput("no frames"))?;
        let mut anim = Vec::new();
        anim.push(match self.kind {
            FrameKind::Pages => 2,
            FrameKind::MixedPages => 3,
            _ => 1,
        });
        put_varint(&mut anim, self.cfg.loop_count as u64);
        put_varint(&mut anim, self.frames.len() as u64 + 1);
        put_varint(&mut anim, d0.num);
        put_varint(&mut anim, d0.den as u64);
        let extra_features = if self.kind == FrameKind::MixedPages {
            Features::EMPTY.with(Feature::MixedPages)
        } else {
            Features::EMPTY
        };
        crate::v4::migrate::finish_sequence(
            first,
            self.frames,
            &[(ANIM, &anim)],
            extra_features,
            self.cfg.coding.checksum_algorithm(),
            stop,
        )
    }
    pub(crate) fn finish_native_checked(mut self, stop: &dyn Stop) -> Result<Vec<u8>, Error> {
        stop.check().map_err(|_| Error::Cancelled)?;
        self.prev_src = Vec::new();
        let (first, d0) = self.first.take().ok_or(Error::InvalidInput("no frames"))?;
        // The outer header declares every feature a later frame or page
        // needs (its mask and what its fields imply, SPEC 0.1, 1.3), and
        // `mixed_pages` for kind 3, so a decoder without one rejects the
        // file at the header; what the outer fields imply is not written.
        let mut later = self.later_features;
        if self.kind == FrameKind::MixedPages {
            later = later.with(Feature::MixedPages);
        } else {
            for f in &self.frames {
                stop.check().map_err(|_| Error::Cancelled)?;
                if let Some(file) = frame_file(f, crate::parse::header::HEADER_VERSION)? {
                    later = later.union(declared(file)?);
                }
            }
        }
        let first = container::require_features(first, later)?;
        let mut anim = Vec::new();
        anim.push(match self.kind {
            FrameKind::Pages => 2,
            FrameKind::MixedPages => 3,
            _ => 1,
        });
        put_varint(&mut anim, self.cfg.loop_count as u64);
        put_varint(&mut anim, self.frames.len() as u64 + 1);
        put_varint(&mut anim, d0.num);
        put_varint(&mut anim, d0.den as u64);
        let mut out = container::insert_before_data(&first, &[(ANIM, &anim)])?;
        drop(first);
        let extra = self
            .frames
            .iter()
            .try_fold(0usize, |acc, f| {
                let chunk = 4usize
                    .checked_add(crate::varint_len(f.len() as u64 + 1))?
                    .checked_add(f.len())?;
                acc.checked_add(chunk)
            })
            .ok_or(Error::TooLarge)?;
        out.try_reserve_exact(extra).map_err(|_| Error::TooLarge)?;
        for f in self.frames {
            stop.check().map_err(|_| Error::Cancelled)?;
            container::put_chunk(&mut out, &FRAM, &f);
        }
        let out = container::finish_unit_table(out)?;
        stop.check().map_err(|_| Error::Cancelled)?;
        let out = crate::checksum::finish_file(out, self.cfg.coding.checksum_algorithm())?;
        stop.check().map_err(|_| Error::Cancelled)?;
        Ok(out)
    }
}

/// Whether `coding` performs pixel-level inter prediction (or enforces an
/// inter-only rejection) from a reference frame on `info`, rather than relying
/// solely on changed-rectangle cropping.
#[cfg(feature = "encode")]
fn uses_pixel_reference(coding: &Lowered, info: &ImageInfo) -> bool {
    match coding {
        Lowered::Lossless(_) => info.sample.bytes() <= 4,
        Lowered::Lossy(c) => {
            #[cfg(feature = "lab")]
            if c.rdoq_lambda != 0.0 {
                return true;
            }
            c.small_transforms || info.channels == crate::Channels::Rgb
        }
        Lowered::Quick(_) => false,
    }
}

/// Bounding box (x0, y0, x1, y1) of the pixels that differ; `None` if none.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "encode")]
fn changed_rect(
    a: &[u8],
    a_stride: usize,
    b: &[u8],
    b_stride: usize,
    w: usize,
    h: usize,
    bpp: usize,
    stop: &dyn Stop,
) -> Result<Option<(usize, usize, usize, usize)>, Error> {
    let rb = w * bpp;
    let (mut x0, mut x1, mut y0, mut y1) = (w, 0, h, 0);
    for y in 0..h {
        stop.check().map_err(|_| Error::Cancelled)?;
        let ra = &a[y * a_stride..y * a_stride + rb];
        let rr = &b[y * b_stride..y * b_stride + rb];
        if ra == rr {
            continue;
        }
        if y1 == 0 {
            y0 = y;
        }
        y1 = y + 1;
        if x0 > 0 {
            let prefix = x0 * bpp;
            if let Some(byte) = ra[..prefix]
                .iter()
                .zip(&rr[..prefix])
                .position(|(p, q)| p != q)
            {
                x0 = byte / bpp;
                if x1 <= x0 {
                    x1 = x0 + 1;
                }
            }
        }
        if x1 < w {
            let start = x1 * bpp;
            if let Some(rel) = ra[start..]
                .iter()
                .zip(&rr[start..])
                .rposition(|(p, q)| p != q)
            {
                x1 = (start + rel) / bpp + 1;
            }
        }
        // Once the horizontal span already covers the full canvas width, only
        // the bottom-most differing row remains to be found.
        if x0 == 0 && x1 == w {
            for by in (y + 1..h).rev() {
                stop.check().map_err(|_| Error::Cancelled)?;
                let ba = &a[by * a_stride..by * a_stride + rb];
                let br = &b[by * b_stride..by * b_stride + rb];
                if ba != br {
                    y1 = by + 1;
                    break;
                }
            }
            break;
        }
    }
    Ok((y1 > 0).then_some((x0, y0, x1, y1)))
}

/// An inter frame's nested file: `cur` coded against `refr` (lossless: the
/// previous exact canvas under the rectangle; lossy: the previous frame's
/// source there), preserving each buffer's row stride.
#[cfg(feature = "encode")]
fn encode_inter(
    coding: &Lowered,
    cur: &[u8],
    cur_stride: usize,
    info: &ImageInfo,
    refr: &[u8],
    refr_stride: usize,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    let none = Metadata::default();
    match coding {
        Lowered::Quick(c) => crate::quick::encode_frame(cur, cur_stride, info, &none, c, pulse),
        Lowered::Lossless(c) => crate::v1::encode_frame_with_progress(
            cur,
            cur_stride,
            info,
            &none,
            c,
            Some((refr, refr_stride)),
            pulse,
        ),
        Lowered::Lossy(c) => crate::lossy::encode_lossy_frame_with_progress(
            cur,
            cur_stride,
            info,
            &none,
            c,
            Some((refr, refr_stride)),
            pulse,
        ),
    }
}

/// The controls of an `fRAM` payload (SPEC 1.3) and its nested file.
pub(crate) struct FrameControl<'a> {
    pub(crate) x0: u32,
    pub(crate) y0: u32,
    pub(crate) duration: FrameDuration,
    pub(crate) present: u64,
    pub(crate) reference: Option<(u64, crate::Digest)>,
    pub(crate) nested: &'a [u8],
}

/// Reads the controls of an `fRAM` payload: unknown presence bits and
/// unknown critical frame records are `Unsupported`, ignorable records are
/// skipped (none are defined).
/// `version` is the outer header's: frame record ids are numbered for it
/// (SPEC Appendix V3), and the bare eight-byte reference digest was written
/// only under header version 2.
pub(crate) fn frame_control(payload: &[u8], version: u8) -> Result<FrameControl<'_>, Error> {
    let mut r = ByteReader::new(payload);
    let x0 = r.var_u32()?;
    let y0 = r.var_u32()?;
    let duration = FrameDuration {
        num: r.varint()?,
        den: r.var_u32()?,
    };
    let present = r.varint()?;
    if present & !(F_PREVIOUS | F_EMPTY | F_RECORDS) != 0 {
        return Err(Error::Unsupported("unknown frame field"));
    }
    let mut reference = None;
    if present & F_RECORDS != 0 {
        let n = r.varint()?;
        if n > r.remaining() as u64 {
            return Err(Error::Corrupt("frame records past fRAM"));
        }
        head_records(r.bytes(n as usize)?, |id, bytes| {
            let id = crate::registry::frame_id(version, id);
            if id == crate::registry::FRAME_REFERENCE {
                let mut rr = ByteReader::new(bytes);
                let index = rr.varint()?;
                // The digest: an algorithm byte and that algorithm's width of
                // bytes, or (header version 2 only) exactly eight bytes of
                // XXH3-64, written before 2026-10-25.
                let legacy = version == crate::parse::header::HEADER_VERSION_2;
                let hash = if legacy && rr.remaining() == 8 {
                    crate::Digest::from_bytes(crate::HashAlgorithm::Xxh3_64, rr.bytes(8)?)
                } else {
                    let algo = crate::HashAlgorithm::from_wire(rr.u8()?)
                        .ok_or(Error::Unsupported("unknown reference digest algorithm"))?;
                    crate::Digest::from_bytes(algo, rr.bytes(rr.remaining())?)
                }
                .ok_or(Error::Corrupt("frame reference length"))?;
                reference = Some((index, hash));
            } else if id & 1 == 1 {
                return Err(Error::Unsupported("unknown frame record"));
            }
            Ok(())
        })?;
    }
    Ok(FrameControl {
        x0,
        y0,
        duration,
        present,
        reference,
        nested: &payload[r.pos..],
    })
}

/// The nested file of an `fRAM` payload (`None` for an empty frame).
pub(crate) fn frame_file(payload: &[u8], version: u8) -> Result<Option<&[u8]>, Error> {
    let c = frame_control(payload, version)?;
    Ok((c.present & F_EMPTY == 0).then_some(c.nested))
}

/// An `fRAM` payload: x0, y0, duration, presence bits, then the nested
/// file (this encoder writes no frame records).
#[cfg(feature = "encode")]
fn frame_payload(x0: u32, y0: u32, d: FrameDuration, present: u64, file: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(file.len() + 16);
    put_varint(&mut p, x0 as u64);
    put_varint(&mut p, y0 as u64);
    put_varint(&mut p, d.num);
    put_varint(&mut p, d.den as u64);
    put_varint(&mut p, present);
    p.extend_from_slice(file);
    p
}

/// Encoded image location and reconstruction dependency within a sequence.
///
/// Coordinates are relative to the complete outer file, not an archive. The
/// primary image uses the whole file as its view; later nonempty images use
/// their nested file. Empty legacy updates have no encoded image. These hints
/// do not replace outer admission, current metadata or canvas checksum checks.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameSeekInfo {
    pub index: usize,
    pub encoded: Option<Range<u64>>,
    /// Absolute range of the fRAM tag, length and controls before the nested image.
    pub controls: Option<Range<u64>>,
    /// Earlier reconstructed image needed by this item, including an empty update.
    pub reference_index: Option<usize>,
    /// Unified-item reference raster digest, when explicitly present on the wire.
    pub reference_hash: Option<crate::Digest>,
}

/// Decodes the frames or pages of a file sequentially or by explicit selection.
pub struct FrameDecoder<'a> {
    data: Cow<'a, [u8]>,
    canvas: ImageInfo,
    meta: Metadata,
    defaults: Option<Vec<u8>>,
    kind: FrameKind,
    pixelsum: PixelTable,
    pixel_status: crate::pixelsum::PixelChecksumStatus,
    loop_count: u32,
    first_duration: FrameDuration,
    frames: Vec<Range<usize>>,
    /// The outer header's declared features: they cover every frame's and
    /// page's own (SPEC 0.1, 1.3).
    declared: Features,
    /// The outer header's version: it numbers the frame records.
    version: u8,
    /// How every file is parsed (the outer one and each frame or page):
    /// [`crate::Limits::verify_checksum`] (a frame's or page's own `cSUM`
    /// is verified only when set) and, with [`Self::with_limits`], the
    /// limits each is checked against.
    nested: NestedPolicy,
    /// The outer file's parsed image, retained for selecting frame 0 again.
    outer: Option<ImageRecord>,
    /// Index of the current frame + 1 (0 = before the first).
    pos: usize,
    /// The current frame: its header and parsed file (frame 0: the outer
    /// file; none for an empty frame).
    cur: Option<(FrameHeader, Option<ImageRecord>)>,
    /// Mixed pages after the first: the current page's own description.
    cur_page: Option<(ImageInfo, Metadata)>,
    unified: bool,
    reference: Option<(usize, crate::Digest)>,
}

impl<'a> FrameDecoder<'a> {
    /// Parses the file; a `cSUM` file checksum is verified here, before any
    /// frame.
    pub fn new(data: &'a [u8]) -> Result<FrameDecoder<'a>, Error> {
        Self::open(data, true, None)
    }

    /// [`Self::new`] under `limits`, checked against the canvas (or the
    /// first page) here and against each later page as it is reached, each
    /// right after its header; they also set whether a file checksum is
    /// verified ([`crate::Limits::verify_checksum`]).
    pub fn with_limits(data: &'a [u8], limits: &crate::Limits) -> Result<FrameDecoder<'a>, Error> {
        Self::open(data, limits.verify_checksum, Some(*limits))
    }

    fn open(
        data: &'a [u8],
        verify: bool,
        limits: Option<crate::Limits>,
    ) -> Result<FrameDecoder<'a>, Error> {
        Self::open_admitted(data, verify, limits, &crate::parse::limits::Unrestricted)
    }

    fn open_admitted(
        data: &'a [u8],
        verify: bool,
        limits: Option<crate::Limits>,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<FrameDecoder<'a>, Error> {
        // SPEC 0.1: what this build cannot decode is rejected from the
        // header, before the chunks and a checksum are read. The outer
        // header's feature set covers every frame's and page's (checked
        // below). Sample-type support is also checked on each nested header.
        let head = crate::parse::header::read_versioned(
            &mut ByteReader::new(data),
            admission.v4_resume().header_limit(),
        )?;
        let undecodable = crate::parse::header::check_decodable(&head.header, head.declared);
        if head.mixed_pages() && undecodable.is_err() {
            return Err(Error::Unsupported(MIXED_UNDECODABLE));
        }
        undecodable?;
        let nested = NestedPolicy { verify, limits };
        let f = nested.parse_admitted(data, admission)?;
        let (kind, loop_count, _, d0) = sequence_control(&f)?;
        let unified = f.anim.is_some_and(|a| a.first() == Some(&4));
        // Every frame's and page's feature set against the outer one,
        // before any frame is decoded (SPEC 0.1, 1.3): a nested file that
        // needs more than the outer header declares is `Corrupt`. Unknown
        // open controls and header fields are `Unsupported` here too (SPEC
        // 0.3, 1.3). Borrow only the controls and header: nested chunks,
        // checksums, limits and pixels are handled when the frame is read.
        if f.header.version != 4 {
            admit_headers(&f, admission)?;
        }
        let mut meta = f.metadata()?;
        let defaults = meta
            .chunks
            .iter()
            .position(|chunk| chunk.tag == defaults::TAG)
            .map(|i| meta.chunks.remove(i).data);
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(f.frames.len())
            .map_err(|_| Error::TooLarge)?;
        for frame in &f.frames {
            frames.push(span(data, frame)?);
        }
        Ok(FrameDecoder {
            data: Cow::Borrowed(data),
            pixelsum: PixelTable::new(data, f.pixelsum)?,
            pixel_status: f.pixelsum.status(),
            declared: f.declared,
            version: head.header.version,
            nested,
            canvas: f.header.info.clone(),
            meta,
            defaults,
            kind,
            loop_count,
            first_duration: d0,
            frames,
            outer: Some(ImageRecord::new(data, f)?),
            pos: 0,
            cur: None,
            cur_page: None,
            unified,
            reference: None,
        })
    }

    /// Canvas (animation) or first page size and pixel format.
    pub fn info(&self) -> &ImageInfo {
        &self.canvas
    }

    /// The primary image's metadata. Legacy animations and uniform pages share
    /// it; unified items and mixed pages expose their own through
    /// [`Self::frame_metadata`].
    pub fn metadata(&self) -> &Metadata {
        &self.meta
    }

    /// Size, pixel format and color of the current frame or page: what
    /// [`Self::decode_into`] writes (an animation frame: its rectangle in
    /// the canvas format). `None` before the first frame, after the last and
    /// for an empty frame.
    pub fn frame_info(&self) -> Option<ImageInfo> {
        let (fh, _) = self.cur.as_ref()?;
        fh.coding?;
        if let Some((info, _)) = &self.cur_page {
            return Some(info.clone());
        }
        Some(ImageInfo {
            width: fh.width,
            height: fh.height,
            ..self.canvas.clone()
        })
    }

    /// Metadata of the current frame or page: a unified item's or mixed page's
    /// own (ICC, EXIF, XMP, orientation, ...), otherwise the file's. `None` when there
    /// is no current frame.
    pub fn frame_metadata(&self) -> Option<&Metadata> {
        self.cur.as_ref()?;
        Some(match &self.cur_page {
            Some((_, m)) => m,
            None => &self.meta,
        })
    }

    /// Resolve optional sequence metadata for the currently selected item.
    /// The raw metadata accessor and pixel decode never expand these defaults.
    /// `max_default_bytes` caps the default payload and the merged attribute map.
    pub fn resolved_frame_metadata(
        &self,
        max_default_bytes: usize,
    ) -> Result<Option<Metadata>, Error> {
        let Some(mut metadata) = self.frame_metadata().cloned() else {
            return Ok(None);
        };
        if let Some(defaults) = &self.defaults
            && let Some(info) = self.frame_info()
        {
            defaults::resolve(
                defaults,
                &mut metadata,
                &info,
                &self.canvas,
                max_default_bytes,
            )?;
        }
        Ok(Some(metadata))
    }

    pub fn kind(&self) -> FrameKind {
        self.kind
    }

    /// Animation loop count (0 = forever).
    pub fn loop_count(&self) -> u32 {
        self.loop_count
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len() + 1
    }

    /// Inspect an item's byte coordinates and dependency without decoding pixels
    /// or changing the current item. The complete outer file was admitted at open;
    /// nested payloads and their checksums are still checked when selected.
    pub fn frame_seek_info(&self, index: usize) -> Result<FrameSeekInfo, Error> {
        if index >= self.frame_count() {
            return Err(Error::InvalidInput("frame index outside sequence"));
        }
        if index == 0 {
            return Ok(FrameSeekInfo {
                index,
                encoded: Some(0..self.data.len() as u64),
                controls: None,
                reference_index: None,
                reference_hash: None,
            });
        }
        let payload = self.frames[index - 1].clone();
        let control = frame_control(&self.data[payload.clone()], self.version)?;
        let reference_index = if self.unified {
            control.reference.map(|(n, _)| n as usize)
        } else {
            (control.present & F_PREVIOUS != 0).then_some(index - 1)
        };
        let encoded = if control.present & F_EMPTY != 0 {
            None
        } else {
            let range = span(&self.data, control.nested)?;
            Some(range.start as u64..range.end as u64)
        };
        let chunk_header_len = 4 + crate::varint_len(payload.len() as u64 + 1);
        let chunk_start = payload
            .start
            .checked_sub(chunk_header_len)
            .ok_or(Error::Corrupt("indexed fRAM start"))?;
        let controls = encoded
            .as_ref()
            .map(|range| chunk_start as u64..range.start);
        Ok(FrameSeekInfo {
            index,
            encoded,
            controls,
            reference_index,
            reference_hash: control.reference.map(|(_, hash)| hash),
        })
    }

    /// Select an item without reconstructing the intervening images.
    ///
    /// This uses the same nested admission and renderer as [`Self::next_frame`].
    /// The caller supplies the earlier reconstructed image named by
    /// [`Self::reference_index`] before decoding; this method creates no raster
    /// cache. Legacy updates require their previous complete canvas. Subsequent
    /// `next_frame` continues immediately after the selected item. An invalid
    /// ordinal leaves the current selection unchanged; other failures clear it.
    pub fn seek_frame(&mut self, index: usize) -> Result<FrameHeader, Error> {
        if index >= self.frame_count() {
            return Err(Error::InvalidInput("frame index outside sequence"));
        }
        self.pos = index;
        self.next_frame()?
            .ok_or(Error::Corrupt("missing selected frame"))
    }

    /// Advances to the next frame; `None` after the last.
    pub fn next_frame(&mut self) -> Result<Option<FrameHeader>, Error> {
        self.next_frame_admitted(&crate::parse::limits::Unrestricted)
    }

    pub(crate) fn next_frame_admitted(
        &mut self,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Option<FrameHeader>, Error> {
        admission.check()?;
        // A failed advance must not leave a previously decoded frame current.
        self.pixel_status = self.pixelsum.view(&self.data).status();
        if let Some((header, Some(file))) = self.cur.take()
            && header.index == 0
        {
            self.outer = Some(file);
        }
        self.cur_page = None;
        self.reference = None;
        let i = self.pos;
        if i > self.frames.len() {
            self.cur = None;
            return Ok(None);
        }
        self.pos += 1;
        if i == 0 {
            let f = self
                .outer
                .take()
                .ok_or(Error::InvalidInput("no primary frame"))?;
            let fh = FrameHeader {
                index: 0,
                x0: 0,
                y0: 0,
                width: f.header.info.width,
                height: f.header.info.height,
                duration: self.first_duration,
                uses_previous: false,
                coding: Some(f.header.coding),
            };
            self.cur = Some((fh, Some(f)));
            return Ok(Some(fh));
        }
        let FrameControl {
            x0,
            y0,
            duration: d,
            present,
            reference,
            nested,
        } = frame_control(&self.data[self.frames[i - 1].clone()], self.version)?;
        if d.den == 0 {
            return Err(Error::Corrupt("bad frame header"));
        }
        if self.unified {
            let file = self.nested.parse_decodable_admitted(nested, admission)?;
            if file.anim.is_some() || !file.frames.is_empty() {
                return Err(Error::Corrupt("sequence item holds frames"));
            }
            let metadata = file.metadata()?;
            let header = FrameHeader {
                index: i,
                x0: 0,
                y0: 0,
                width: file.header.info.width,
                height: file.header.info.height,
                duration: d,
                uses_previous: reference.is_some_and(|(n, _)| n == i as u64 - 1),
                coding: Some(file.header.coding),
            };
            self.reference = reference.map(|(index, hash)| (index as usize, hash));
            self.cur_page = Some((file.header.info.clone(), metadata));
            self.cur = Some((header, Some(ImageRecord::new(&self.data, file)?)));
            return Ok(Some(header));
        }
        if self.kind == FrameKind::MixedPages {
            let nested = span(&self.data, nested)?;
            return self.next_mixed_page(i, x0, y0, d, present, nested, admission);
        }
        let uses_previous = present & F_PREVIOUS != 0;
        let (w, h, coding, f) = if present & F_EMPTY != 0 {
            if !uses_previous || !nested.is_empty() {
                return Err(Error::Corrupt("bad empty frame"));
            }
            (0, 0, None, None)
        } else {
            // The frame's file under the decoder's policy: its limits right
            // after its header, its own `cSUM` when verifying (SPEC 1.4).
            let f = self.nested.parse_decodable_admitted(nested, admission)?;
            let h = &f.header;
            if !self.declared.covers(f.declared) {
                return Err(Error::Corrupt(FRAME_UNDECLARED));
            }
            if h.info.channels != self.canvas.channels
                || h.info.sample != self.canvas.sample
                || (0..h.info.channels.count())
                    .any(|c| h.info.channel_bits(c) != self.canvas.channel_bits(c))
            {
                return Err(Error::Corrupt("frame pixel format differs from the canvas"));
            }
            (h.info.width, h.info.height, Some(h.coding), Some(f))
        };
        match self.kind {
            FrameKind::Animation => {
                let fits = x0.checked_add(w).is_some_and(|x| x <= self.canvas.width)
                    && y0.checked_add(h).is_some_and(|y| y <= self.canvas.height);
                let full = x0 == 0 && y0 == 0 && w == self.canvas.width && h == self.canvas.height;
                if !fits || (!uses_previous && !full) {
                    return Err(Error::Corrupt("frame rectangle"));
                }
            }
            _ => {
                if x0 != 0 || y0 != 0 || uses_previous {
                    return Err(Error::Corrupt("page with a rectangle"));
                }
            }
        }
        let fh = FrameHeader {
            index: i,
            x0,
            y0,
            width: w,
            height: h,
            duration: d,
            uses_previous,
            coding,
        };
        self.cur = Some((
            fh,
            f.map(|file| ImageRecord::new(&self.data, file))
                .transpose()?,
        ));
        Ok(Some(fh))
    }

    /// A mixed page after the first: a complete file that describes itself.
    fn next_mixed_page(
        &mut self,
        i: usize,
        x0: u32,
        y0: u32,
        d: FrameDuration,
        present: u64,
        nested: Range<usize>,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Option<FrameHeader>, Error> {
        // A frame record list is allowed; a page never uses the previous one.
        if x0 != 0 || y0 != 0 || present & (F_PREVIOUS | F_EMPTY) != 0 {
            return Err(Error::Corrupt("page with a rectangle"));
        }
        let f = self.nested.parse_admitted(&self.data[nested], admission)?;
        if f.anim.is_some() || !f.frames.is_empty() {
            return Err(Error::Corrupt("page holds frames"));
        }
        // The outer header's feature set covers the page's (`new` checked
        // every page's header), and this build decodes the outer set.
        if !self.declared.covers(f.declared) {
            return Err(Error::Corrupt(PAGE_UNDECLARED));
        }
        let meta = f.metadata()?;
        let fh = FrameHeader {
            index: i,
            x0: 0,
            y0: 0,
            width: f.header.info.width,
            height: f.header.info.height,
            duration: d,
            uses_previous: false,
            coding: Some(f.header.coding),
        };
        self.cur_page = Some((f.header.info.clone(), meta));
        self.cur = Some((fh, Some(ImageRecord::new(&self.data, f)?)));
        Ok(Some(fh))
    }

    /// Ordinal of the current item's reference. Legacy animation updates use
    /// their predecessor; unified items can select any earlier compatible image.
    pub fn reference_index(&self) -> Option<usize> {
        if self.unified {
            self.reference.map(|(index, _)| index)
        } else {
            self.cur.as_ref().and_then(|(h, _)| {
                h.uses_previous
                    .then_some(h.index)
                    .and_then(|i| i.checked_sub(1))
            })
        }
    }

    /// Render using a caller-owned reference, preserving independent row strides.
    /// Unified references are checked against the declared decoded-raster hash
    /// before any output is written. Without a reference, use `decode_into`.
    pub fn decode_into_with_reference(
        &mut self,
        reference: &[u8],
        reference_stride: usize,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        self.pixel_status = self.pixelsum.view(&self.data).status();
        if !self.unified {
            return Err(Error::InvalidInput(
                "explicit reference needs a unified sequence",
            ));
        }
        let (index, hash) = self
            .reference
            .ok_or(Error::InvalidInput("current item is independent"))?;
        let info = self
            .frame_info()
            .ok_or(Error::InvalidInput("no current frame"))?;
        let needed = info
            .checked_stride_len(stride)
            .ok_or(Error::InvalidInput("invalid image layout"))?;
        if out.len() < needed {
            return Err(Error::InvalidInput("output buffer too small"));
        }
        if crate::pixelsum::digest_with(
            hash.algorithm(),
            &info,
            index as u64,
            reference,
            reference_stride,
        )? != hash
        {
            return Err(Error::Corrupt("reference raster checksum mismatch"));
        }
        let row = info.row_bytes();
        for y in 0..info.height as usize {
            out[y * stride..y * stride + row]
                .copy_from_slice(&reference[y * reference_stride..y * reference_stride + row]);
        }
        self.render_current(out, stride, threads)
    }

    /// Decoded bitmap coverage of the current full frame or page.
    pub fn pixel_checksum_status(&self) -> crate::pixelsum::PixelChecksumStatus {
        self.pixel_status
    }

    /// Decodes the current item into rows of `stride` bytes. When
    /// [`Self::reference_index`] returns an ordinal, `out` must initially hold
    /// that decoded image, which need not be the preceding item. Legacy
    /// animation updates keep their canvas semantics; independent pages replace
    /// the output. Use [`Self::decode_into_with_reference`] for separate
    /// reference/output buffers with unified items.
    /// Verifies its full canvas/page checksum when present.
    /// Empty animation frames still verify the caller's entire canvas.
    pub fn decode_into(
        &mut self,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        self.pixel_status = self.pixelsum.view(&self.data).status();
        if self.unified
            && let Some((index, hash)) = self.reference
        {
            let info = self
                .frame_info()
                .ok_or(Error::InvalidInput("no current frame"))?;
            if crate::pixelsum::digest_with(hash.algorithm(), &info, index as u64, out, stride)?
                != hash
            {
                return Err(Error::Corrupt("reference raster checksum mismatch"));
            }
        }
        self.render_current(out, stride, threads)
    }

    fn render_current(
        &mut self,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        if !matches!(self.pixelsum, PixelTable::Known(..)) {
            return self.decode_current_into(out, stride, threads);
        }
        self.decode_current_into(out, stride, threads)?;
        let (fh, _) = self
            .cur
            .as_ref()
            .ok_or(Error::InvalidInput("no current frame"))?;
        let info = if self.kind == FrameKind::Animation {
            self.canvas.clone()
        } else {
            self.frame_info().ok_or(Error::Corrupt("pSUM page info"))?
        };
        self.pixelsum
            .view(&self.data)
            .verify(fh.index, &info, out, stride)?;
        if matches!(self.pixelsum, PixelTable::Known(..)) {
            self.pixel_status = crate::pixelsum::PixelChecksumStatus::Verified;
        }
        Ok(())
    }

    fn decode_current_into(
        &mut self,
        out: &mut [u8],
        stride: usize,
        threads: usize,
    ) -> Result<(), Error> {
        self.decode_current_with(out, stride, |file, out, stride, inter, inherited| {
            crate::v1::decode_parsed_frame(file, out, stride, threads, inter, inherited)
        })
    }

    fn decode_current_with(
        &self,
        out: &mut [u8],
        stride: usize,
        mut decode: impl FnMut(
            &container::File<'_>,
            &mut [u8],
            usize,
            bool,
            Option<&ImageInfo>,
        ) -> Result<ImageInfo, Error>,
    ) -> Result<(), Error> {
        let (fh, file) = self
            .cur
            .as_ref()
            .ok_or(Error::InvalidInput("no current frame"))?;
        let Some(record) = file else {
            return Ok(());
        };
        let file = &record.view(&self.data);
        if self.unified {
            return decode(file, out, stride, self.reference.is_some(), None).map(|_| ());
        }
        if self.kind == FrameKind::MixedPages {
            // Each page in its own format and color: nothing is inherited.
            return decode(file, out, stride, false, None).map(|_| ());
        }
        let bpp = self.canvas.row_bytes() / self.canvas.width as usize;
        let at = (fh.y0 as usize)
            .checked_mul(stride)
            .and_then(|row| {
                (fh.x0 as usize)
                    .checked_mul(bpp)
                    .and_then(|col| row.checked_add(col))
            })
            .ok_or(Error::InvalidInput("frame offset overflow"))?;
        if at > out.len() {
            return Err(Error::InvalidInput("output buffer too small"));
        }
        decode(
            file,
            &mut out[at..],
            stride,
            fh.uses_previous,
            Some(&self.canvas),
        )
        .map(|_| ())
    }
}

/// The constructor and encoded-domain migration share control/capability
/// admission without decoding pixels or copying metadata payloads.
pub(crate) fn admit_headers(
    f: &crate::container::File<'_>,
    admission: &impl crate::parse::limits::Admission,
) -> Result<(), Error> {
    let (kind, _, _, _) = sequence_control(f)?;
    let unified = f.anim.is_some_and(|a| a.first() == Some(&4));
    for (ordinal, payload) in f.frames.iter().enumerate() {
        admission.check()?;
        let control = frame_control(payload, f.header.version)?;
        if !unified && control.reference.is_some() {
            return Err(Error::Corrupt("image reference outside unified sequence"));
        }
        if unified
            && (control.x0 != 0
                || control.y0 != 0
                || control.present & (F_PREVIOUS | F_EMPTY) != 0
                || control.duration.den == 0)
        {
            return Err(Error::Corrupt("unified item controls"));
        }
        let Some(p) = frame_file(payload, f.header.version)? else {
            continue;
        };
        let hp = crate::parse::header::read_versioned(
            &mut ByteReader::new(p),
            admission.v4_resume().header_limit(),
        )?;
        if unified {
            if kind == FrameKind::Animation && hp.header.info != f.header.info {
                return Err(Error::Corrupt("animation canvas format changes"));
            }
            if let Some((reference, _)) = control.reference {
                if reference > ordinal as u64 {
                    return Err(Error::Corrupt("reference is not an earlier image"));
                }
                let earlier = if reference == 0 {
                    f.header.info.clone()
                } else {
                    let bytes = frame_file(f.frames[reference as usize - 1], f.header.version)?
                        .ok_or(Error::Corrupt("reference has no image"))?;
                    crate::parse::header::read_versioned(
                        &mut ByteReader::new(bytes),
                        admission.v4_resume().header_limit(),
                    )?
                    .header
                    .info
                };
                if earlier != hp.header.info {
                    return Err(Error::Corrupt("reference pixel description differs"));
                }
            }
        }
        if !f.declared.covers(hp.declared) {
            return Err(Error::Corrupt(match kind {
                FrameKind::MixedPages => PAGE_UNDECLARED,
                _ => FRAME_UNDECLARED,
            }));
        }
        // Admit each nested sample type before exposing an earlier frame.
        if admission.decodes()
            && let Err(e) = crate::parse::header::check_decodable(&hp.header, hp.declared)
        {
            return Err(if kind == FrameKind::MixedPages {
                Error::Unsupported(MIXED_UNDECODABLE)
            } else {
                e
            });
        }
    }
    Ok(())
}

/// Shared container-level sequence grammar; does not decode nested pixels.
pub(crate) fn sequence_control(
    f: &crate::container::File<'_>,
) -> Result<(FrameKind, u32, u64, FrameDuration), Error> {
    sequence_control_parts(f.anim, f.frames.len(), f.declared, false)
}

pub(crate) fn sequence_control_parts(
    anim: Option<&[u8]>,
    frames: usize,
    declared: Features,
    strict: bool,
) -> Result<(FrameKind, u32, u64, FrameDuration), Error> {
    let (kind, loop_count, count, d0) = match anim {
        None => (FrameKind::Still, 0, 1, FrameDuration::ZERO),
        Some(a) => {
            let mut r = ByteReader::new(a);
            let wire_kind = r.u8()?;
            let mut kind = match wire_kind {
                1 => FrameKind::Animation,
                2 => FrameKind::Pages,
                3 | 4 => FrameKind::MixedPages,
                _ => return Err(Error::Unsupported("unknown aNIM kind")),
            };
            let lc = r.varint()?;
            let n = r.varint()?;
            let d = FrameDuration {
                num: r.varint()?,
                den: r.var_u32()?,
            };
            if d.den == 0 || lc > u32::MAX as u64 {
                return Err(Error::Corrupt("bad aNIM"));
            }
            if wire_kind == 4 {
                if !declared.contains(Feature::SequenceItems) {
                    return Err(Error::Corrupt(
                        "aNIM kind 4 needs the feature sequence_items",
                    ));
                }
                kind = match r.u8()? {
                    0 => FrameKind::MixedPages,
                    1 => FrameKind::Animation,
                    _ => return Err(Error::Unsupported("unknown sequence presentation")),
                };
            }
            if (strict || kind == FrameKind::MixedPages || wire_kind == 4) && r.remaining() != 0 {
                return Err(Error::Corrupt("bad aNIM"));
            }
            (kind, lc as u32, n, d)
        }
    };
    if count != frames as u64 + 1 {
        return Err(Error::Corrupt("frame count does not match the fRAM chunks"));
    }
    if kind == FrameKind::Still && frames != 0 {
        return Err(Error::Corrupt("fRAM chunks without aNIM"));
    }
    Ok((kind, loop_count, count, d0))
}
