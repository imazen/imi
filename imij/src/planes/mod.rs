//! Named, independently typed scalar planes and explicitly supplied levels.
//!
//! An existing native IMI primary remains the ordinary decode fallback. Each
//! added level is independently lossless or lossy and uses independent native
//! tiles: scalar Gray or shared packed color. No level is reconstructed from
//! another and no tile needs a
//! neighboring tile. Lossless levels preserve supplied sample bit patterns;
//! lossy levels use the existing codec's numeric/color assumptions and may
//! show tile seams. A supplied level is not a promise about a resizer or about
//! lossy output matching its reference exactly. RAW roles require lossless.
//!
//! Calibration, vendor RAW/DNG and associated video/audio are opaque bounded
//! attachments. The library neither interprets nor recursively decodes them.
//! Scalar sample buffers use the existing SampleType native-endian convention.
//! Rectangle coordinates are local to a selected level, independent of a
//! plane's signed registration origin. Arbitrary level dimensions are allowed.
//! The ordinary primary may be a thumbnail; indexed ROI applies to added tiled
//! levels, not to the primary's existing stream. Color component planes share
//! native RGB/RGBA storage rather than duplicating samples as separate Gray files.

use crate::science::ScienceRecords;
use crate::{Channels, Cicp, Coding, Error, SampleType};
use alloc::{string::String, vec::Vec};

mod decode;
mod encode;
#[cfg(all(feature = "std", feature = "encode"))]
mod forward;
#[cfg(all(test, feature = "std", feature = "encode", not(target_family = "wasm")))]
mod forward_tests;
mod io;
#[cfg(feature = "encode")]
mod levels;
mod ownership;
#[cfg(all(test, feature = "std", feature = "encode"))]
mod page_tests;
#[cfg(all(test, feature = "encode"))]
mod precision_tests;
mod record_pages;
mod region_plan;
#[cfg(all(feature = "std", feature = "encode"))]
pub use forward::PlaneSetWriter;
#[cfg(all(test, feature = "std", feature = "encode"))]
mod view_tests;
mod views;
mod wire;
pub use views::{
    AxisRegistration, ImageDirectory, ImageView, ImageViewRole, ViewBacking, ViewRegistration,
};

#[cfg(feature = "std")]
pub(crate) use decode::validate_all_tiles;
pub use decode::{ChecksumCoverage, PlaneSetReader};
#[cfg(feature = "encode")]
pub use encode::PlaneSetBuilder;
#[cfg(test)]
pub(crate) use encode::upgrade_image_planes;
pub(crate) use encode::upgrade_image_planes_with;
#[cfg(feature = "std")]
pub use io::SeekReader;
pub use io::{ReadAt, SliceReader, ZencodecReadAt};
#[cfg(feature = "encode")]
pub use levels::OriginalPlane;

/// Stable within this plane set; obtained from the builder or reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneId(u32);
impl PlaneId {
    pub fn index(self) -> u32 {
        self.0
    }
}
/// Stable within this plane set; attachment contents remain opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachmentId(u32);
impl AttachmentId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaneRole {
    Auxiliary,
    Luma,
    Red,
    Green,
    Blue,
    Alpha,
    Depth,
    Disparity,
    Confidence,
    PortraitMatte,
    Segmentation,
    Raw,
    /// The real part of a complex plane pair (role byte 12): one per group,
    /// with a [`PlaneRole::ComplexImaginary`] plane of the same sample type,
    /// precision and level sizes. Complex levels are lossless; a packed
    /// GrayAlpha level (real, imaginary) decodes to interleaved complex
    /// samples with `PlaneSetReader::decode_color_region`.
    ComplexReal,
    /// The imaginary part of a complex plane pair (role byte 13).
    ComplexImaginary,
    /// A non-color numeric plane (role byte 14).
    Scalar,
    Custom(String),
}

impl PlaneRole {
    #[cfg(feature = "encode")]
    fn single_channel(&self) -> Channels {
        if *self == Self::Scalar {
            Channels::Scalar
        } else {
            Channels::Gray
        }
    }
    fn is_complex(&self) -> bool {
        matches!(self, PlaneRole::ComplexReal | PlaneRole::ComplexImaginary)
    }
}

/// The complex plane convention (docs/PLANES_AND_ROI.md, "Complex
/// planes"): in each group a `ComplexReal` and a `ComplexImaginary` plane,
/// both or neither, with one sample type and precision, equal level sizes
/// and lossless levels.
pub(crate) fn check_complex_pairs(planes: &[PlaneDescriptor]) -> Result<(), &'static str> {
    for p in planes.iter().filter(|p| p.role.is_complex()) {
        if p.levels.iter().any(|l| l.coding != Coding::Lossless) {
            return Err("lossy complex plane level");
        }
        if planes
            .iter()
            .filter(|q| q.group == p.group && q.role == p.role)
            .count()
            != 1
        {
            return Err("two complex planes of one part in a group");
        }
        let partner = planes
            .iter()
            .find(|q| q.group == p.group && q.role.is_complex() && q.role != p.role)
            .ok_or("complex plane without its partner")?;
        if partner.sample != p.sample
            || partner.precision != p.precision
            || partner.levels.len() != p.levels.len()
            || partner
                .levels
                .iter()
                .zip(&p.levels)
                .any(|(a, b)| (a.width, a.height) != (b.width, b.height))
        {
            return Err("complex plane pair type or level sizes differ");
        }
    }
    Ok(())
}

/// A declaration of how the supplied reference pixels were obtained.
/// It creates no decoding dependency and is not verified by the codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LevelSource {
    SuppliedReference,
    DirectFromOriginal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaneDescriptor {
    name: String,
    sample: SampleType,
    precision: u8,
    role: PlaneRole,
    origin: (i64, i64),
    group: u32,
    calibration: Option<AttachmentId>,
    units: Option<String>,
    cicp: Cicp,
    icc_profile: Option<AttachmentId>,
    levels: Vec<LevelDescriptor>,
}
impl PlaneDescriptor {
    pub fn new(name: impl Into<String>, sample: SampleType) -> Self {
        Self {
            name: name.into(),
            sample,
            precision: sample.depth() as u8,
            role: PlaneRole::Auxiliary,
            origin: (0, 0),
            group: 0,
            calibration: None,
            units: None,
            cicp: Cicp::SRGB,
            icc_profile: None,
            levels: Vec::new(),
        }
    }
    pub fn with_role(mut self, role: PlaneRole) -> Self {
        if role == PlaneRole::Scalar {
            self.cicp = Cicp::UNSPECIFIED;
        }
        self.role = role;
        self
    }
    pub fn with_origin(mut self, x: i64, y: i64) -> Self {
        self.origin = (x, y);
        self
    }
    pub fn with_group(mut self, group: u32) -> Self {
        self.group = group;
        self
    }
    pub fn with_calibration(mut self, id: AttachmentId) -> Self {
        self.calibration = Some(id);
        self
    }
    /// Exact application-defined units, for example `m`, `1/m` or `normalized`.
    pub fn with_units(mut self, units: impl Into<String>) -> Self {
        self.units = Some(units.into());
        self
    }
    /// Independent color declaration for this plane/group. Numerical auxiliary
    /// roles do not acquire display color semantics from this field. No sample
    /// conversion or ICC interpretation occurs. Lossy float samples must be
    /// linear-light (transfer 8); U8 XYB requires sRGB transfer 13.
    pub fn with_cicp(mut self, cicp: Cicp) -> Self {
        self.cicp = cicp;
        self
    }
    /// Opaque original ICC bytes in a separately bounded attachment. The caller
    /// is responsible for consistency between this profile and declared CICP.
    pub fn with_icc_profile(mut self, id: AttachmentId) -> Self {
        self.icc_profile = Some(id);
        self
    }
    pub fn cicp(&self) -> Cicp {
        self.cicp
    }
    pub fn icc_profile(&self) -> Option<AttachmentId> {
        self.icc_profile
    }
    /// Right-aligned significant integer bits for newly encoded native levels.
    /// Lossy normalization uses this range; lossless coding preserves storage
    /// bits exactly. Floats retain their complete 16/32-bit representation.
    pub fn with_precision(mut self, bits: u8) -> Self {
        self.precision = bits;
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn sample_type(&self) -> SampleType {
        self.sample
    }
    pub fn precision_bits(&self) -> u8 {
        self.precision
    }
    pub fn role(&self) -> &PlaneRole {
        &self.role
    }
    /// Registration origin in the first supplied level's reference grid.
    /// Other levels carry dimensions, not an inferred pixel-center mapping.
    pub fn origin(&self) -> (i64, i64) {
        self.origin
    }
    pub fn reference_size(&self) -> Option<(u32, u32)> {
        self.levels.first().map(|l| (l.width, l.height))
    }
    pub fn group(&self) -> u32 {
        self.group
    }
    pub fn calibration(&self) -> Option<AttachmentId> {
        self.calibration
    }
    pub fn units(&self) -> Option<&str> {
        self.units.as_deref()
    }
    pub fn levels(&self) -> &[LevelDescriptor] {
        &self.levels
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelDescriptor {
    width: u32,
    height: u32,
    tile_width: u32,
    tile_height: u32,
    coding: Coding,
    source: LevelSource,
    channels: Channels,
    component: u8,
    record_start: u32,
}
impl LevelDescriptor {
    pub fn new(width: u32, height: u32, tile_width: u32, tile_height: u32) -> Self {
        Self {
            width,
            height,
            tile_width,
            tile_height,
            coding: Coding::Lossless,
            source: LevelSource::SuppliedReference,
            channels: Channels::Gray,
            component: 0,
            record_start: 0,
        }
    }
    pub fn with_source(mut self, source: LevelSource) -> Self {
        self.source = source;
        self
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn tile_width(&self) -> u32 {
        self.tile_width
    }
    pub fn tile_height(&self) -> u32 {
        self.tile_height
    }
    pub fn coding(&self) -> Coding {
        self.coding
    }
    pub fn source(&self) -> LevelSource {
        self.source
    }
    /// Native shared tile layout. Scalar levels use Gray; color levels retain
    /// optimized RGB/RGBA coding and share one tile between their components.
    pub fn tile_channels(&self) -> Channels {
        self.channels
    }
    pub fn component(&self) -> u8 {
        self.component
    }
    fn tiles(&self) -> Result<(u32, u32, usize), Error> {
        if self.width == 0 || self.height == 0 || self.tile_width == 0 || self.tile_height == 0 {
            return Err(Error::InvalidInput("zero plane level geometry"));
        }
        let nx = self.width.div_ceil(self.tile_width);
        let ny = self.height.div_ceil(self.tile_height);
        let count = usize::try_from(u64::from(nx) * u64::from(ny)).map_err(|_| Error::TooLarge)?;
        Ok((nx, ny, count))
    }
}

/// Strict, nonempty ROI in local level coordinates; no implicit clipping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}
impl Rect {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn x(self) -> u32 {
        self.x
    }
    pub fn y(self) -> u32 {
        self.y
    }
    pub fn width(self) -> u32 {
        self.width
    }
    pub fn height(self) -> u32 {
        self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttachmentTiming {
    start_num: i64,
    start_den: u32,
    duration_num: u64,
    duration_den: u32,
}
impl AttachmentTiming {
    /// Exact seconds relative to the associated capture group's time origin.
    pub fn new(
        start_num: i64,
        start_den: u32,
        duration_num: u64,
        duration_den: u32,
    ) -> Result<Self, Error> {
        if start_den == 0 || duration_den == 0 {
            return Err(Error::InvalidInput("zero attachment time denominator"));
        }
        Ok(Self {
            start_num,
            start_den,
            duration_num,
            duration_den,
        })
    }
    pub fn start(self) -> (i64, u32) {
        (self.start_num, self.start_den)
    }
    pub fn duration(self) -> (u64, u32) {
        (self.duration_num, self.duration_den)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentDescriptor {
    name: String,
    mime: String,
    group: u32,
    timing: Option<AttachmentTiming>,
    record: u32,
}
impl AttachmentDescriptor {
    pub fn new(name: impl Into<String>, mime: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mime: mime.into(),
            group: 0,
            timing: None,
            record: 0,
        }
    }
    pub fn with_group(mut self, group: u32) -> Self {
        self.group = group;
        self
    }
    pub fn with_timing(mut self, timing: AttachmentTiming) -> Self {
        self.timing = Some(timing);
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn mime_type(&self) -> &str {
        &self.mime
    }
    pub fn group(&self) -> u32 {
        self.group
    }
    pub fn timing(&self) -> Option<AttachmentTiming> {
        self.timing
    }
}

/// Explicit index, record, allocation-size and per-request work bounds.
/// These do not promise an RSS cap for the existing native codec's scratch.
#[derive(Clone, Copy, Debug)]
pub struct PlaneSetLimits {
    v4_profile: Option<crate::v4::CodingProfile>,
    max_index_bytes: usize,
    max_planes: usize,
    max_levels: usize,
    max_views: usize,
    max_records: usize,
    max_attachments: usize,
    max_label_bytes: usize,
    max_record_bytes: u64,
    max_tile_bytes: u64,
    max_attachment_bytes: u64,
    max_file_bytes: u64,
    max_plane_pixels: u64,
    max_compressed_work: u64,
    max_decoded_work: u64,
    decode_workers: usize,
    verify_checksum: bool,
    verify_scoped_pixels: bool,
    checksum_round_out: bool,
}
impl Default for PlaneSetLimits {
    fn default() -> Self {
        Self {
            v4_profile: None,
            max_index_bytes: 4 << 20,
            max_planes: 1_024,
            max_levels: 1024,
            max_views: 1024,
            max_records: 1 << 20,
            max_attachments: 128,
            max_label_bytes: 256,
            max_record_bytes: 64 << 20,
            max_tile_bytes: 64 << 20,
            max_attachment_bytes: 1 << 30,
            max_file_bytes: 1 << 46,
            max_plane_pixels: 1 << 40,
            max_compressed_work: 1 << 30,
            max_decoded_work: 1 << 30,
            decode_workers: 1,
            verify_checksum: true,
            verify_scoped_pixels: true,
            checksum_round_out: true,
        }
    }
}
impl PlaneSetLimits {
    /// Require this profile for every image view or plane tile decoded through
    /// this reader. Directory inspection can still describe other views.
    pub fn with_v4_profile(mut self, profile: crate::v4::CodingProfile) -> Self {
        self.v4_profile = Some(profile);
        self
    }

    /// Request up to this many workers for reconstructing a fetched image tile.
    /// Range reads stay on the owning thread. The default is one worker.
    #[doc(hidden)]
    pub fn with_decode_workers(mut self, workers: usize) -> Self {
        self.decode_workers = workers.max(1);
        self
    }

    /// Verify declared V4 group and tile pixel checks, enabled by default.
    /// See [`crate::decode_resources::DecodePolicy::with_scoped_pixel_checks`].
    #[doc(hidden)]
    pub fn with_scoped_pixel_verification(mut self, verify: bool) -> Self {
        self.verify_scoped_pixels = verify;
        self
    }

    /// Check a file checksum (`cSUM`) on the bytes the reader fetches. On by
    /// default: every byte the reader uses is then verified before it is
    /// used (see [`Self::with_checksum_round_out`]).
    pub fn with_checksum_verification(mut self, verify: bool) -> Self {
        self.verify_checksum = verify;
        self
    }
    /// The piece policy of checksum verification. `true` (the default):
    /// every piece a read overlaps is checked, the rest of each read from
    /// the source and hashed through a buffer of at most 1 MiB (at most one
    /// extra piece at each end of a read; pieces are 64 KiB to 1 MiB in
    /// files this library writes), so every byte the reader uses is
    /// verified. `false`, the partial-piece policy: only pieces that lie
    /// wholly inside a read are checked, so a small read can use bytes that
    /// no checksum verified; [`PlaneSetReader::checksum_coverage`] reports
    /// how many.
    pub fn with_checksum_round_out(mut self, round_out: bool) -> Self {
        self.checksum_round_out = round_out;
        self
    }
    pub fn with_index_bytes(mut self, n: usize) -> Self {
        self.max_index_bytes = n;
        self
    }
    pub fn with_planes(mut self, n: usize) -> Self {
        self.max_planes = n;
        self
    }
    pub fn with_views(mut self, n: usize) -> Self {
        self.max_views = n;
        self
    }
    pub fn with_levels(mut self, n: usize) -> Self {
        self.max_levels = n;
        self
    }
    pub fn with_records(mut self, n: usize) -> Self {
        self.max_records = n;
        self
    }
    pub fn with_attachments(mut self, n: usize) -> Self {
        self.max_attachments = n;
        self
    }
    pub fn with_label_bytes(mut self, n: usize) -> Self {
        self.max_label_bytes = n;
        self
    }
    pub fn with_record_bytes(mut self, n: u64) -> Self {
        self.max_record_bytes = n;
        self
    }
    pub fn with_tile_bytes(mut self, n: u64) -> Self {
        self.max_tile_bytes = n;
        self
    }
    pub fn with_attachment_bytes(mut self, n: u64) -> Self {
        self.max_attachment_bytes = n;
        self
    }
    pub fn with_file_bytes(mut self, n: u64) -> Self {
        self.max_file_bytes = n;
        self
    }
    pub fn with_plane_pixels(mut self, n: u64) -> Self {
        self.max_plane_pixels = n;
        self
    }
    pub fn with_work_bytes(mut self, compressed: u64, decoded: u64) -> Self {
        self.max_compressed_work = compressed;
        self.max_decoded_work = decoded;
        self
    }
    pub fn index_bytes(self) -> usize {
        self.max_index_bytes
    }
    pub fn tile_bytes(self) -> u64 {
        self.max_tile_bytes
    }
}

#[derive(Clone, Copy)]
struct Record {
    offset: u64,
    len: u64,
}
struct Directory {
    images: Option<ImageDirectory>,
    pages: Option<record_pages::RecordPages>,
    primary_len: u64,
    records: Vec<Record>,
    planes: Vec<PlaneDescriptor>,
    attachments: Vec<AttachmentDescriptor>,
}

impl Directory {
    fn record_count(&self) -> usize {
        self.pages
            .as_ref()
            .map_or(self.records.len(), |p| p.count as usize)
    }
}

fn sample_bytes(width: u32, height: u32, sample: SampleType) -> Result<usize, Error> {
    let n = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|v| v.checked_mul(sample.bytes() as u64))
        .ok_or(Error::TooLarge)?;
    usize::try_from(n).map_err(|_| Error::TooLarge)
}
#[cfg(feature = "encode")]
fn extent(width: u32, height: u32, sample: SampleType, stride: usize) -> Result<usize, Error> {
    let row = sample_bytes(width, 1, sample)?;
    if width == 0 || height == 0 || stride < row {
        return Err(Error::InvalidInput("plane stride or geometry"));
    }
    stride
        .checked_mul(height as usize - 1)
        .and_then(|v| v.checked_add(row))
        .ok_or(Error::TooLarge)
}
fn label(text: &str, limits: PlaneSetLimits) -> Result<(), Error> {
    if text.is_empty() || text.as_bytes().contains(&0) {
        return Err(Error::InvalidInput("empty or NUL plane label"));
    }
    if text.len() > limits.max_label_bytes || text.len() > u16::MAX as usize {
        return Err(Error::TooLarge);
    }
    Ok(())
}
fn validate_plane(p: &PlaneDescriptor, limits: PlaneSetLimits) -> Result<(), Error> {
    if p.role == PlaneRole::Scalar && (p.cicp != Cicp::UNSPECIFIED || p.icc_profile.is_some()) {
        return Err(Error::InvalidInput("color profile on scalar plane"));
    }
    label(&p.name, limits)?;
    if let PlaneRole::Custom(name) = &p.role {
        label(name, limits)?;
    }
    if let Some(units) = &p.units {
        label(units, limits)?;
    }
    if p.cicp.matrix_coefficients != 0 {
        return Err(Error::InvalidInput(
            "plane samples must be identity components",
        ));
    }
    if p.precision == 0
        || p.precision as u32 > p.sample.depth()
        || (p.sample.is_float() && p.precision as u32 != p.sample.depth())
    {
        return Err(Error::InvalidInput("plane significant bits"));
    }
    Ok(())
}
fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    out.try_reserve(bytes.len()).map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(bytes);
    Ok(())
}
