//! Science records (SPEC.md 1.5): calibration and missing data, axes and
//! coordinates, categories, and structured metadata.
//!
//! The records describe stored samples; they never change them, and no
//! decoder needs them to produce pixels. In a single file they are ancillary,
//! unsafe-to-copy chunks (`cALB`, `aXES`, `cORD`, `sMDT`, `cATG`) that the
//! container keeps in [`Metadata::chunks`](crate::Metadata) like any other
//! ancillary chunk, so every encoder, decoder, frame and reader path carries
//! them unchanged.
//! [`ScienceRecords::write_metadata`](crate::science::ScienceRecords::write_metadata)
//! and [`ScienceRecords::from_metadata`](crate::science::ScienceRecords::from_metadata)
//! convert between those chunks and the
//! typed records. Plane sets keep per-plane records in one attachment
//! (`PlaneSetBuilder::set_science`, `PlaneSetReader::science`).
//!
//! The typed reader is strict: a malformed record is
//! [`Error::Corrupt`](crate::Error), while a record of an unknown version or
//! structured format is skipped. Image decoding never reads these records.
//!
//! ```
//! use imij::science::{Axes, Axis, AxisBinding, AxisKind, AxisLevel,
//!     ChannelCalibration, ScienceRecords, StructuredFormat, StructuredMetadata};
//! use imij::{Channels, ImageInfo, Metadata, SampleType};
//! # #[cfg(feature = "encode")]
//! use imij::{ImageRef, ImiEncoderConfig};
//!
//! let info = ImageInfo::new(64, 64, Channels::Gray, SampleType::U16);
//! let pixels = vec![0u8; info.row_bytes() * 64];
//! let mut c = ChannelCalibration::default();
//! c.offset = -1024.0;                      // DICOM Rescale Intercept
//! c.quantity = Some("HU".into());          // DICOM Rescale Type
//! let mut axes = Axes::new(vec![
//!     Axis::new("y", AxisKind::Space, AxisBinding::Rows),
//!     Axis::new("x", AxisKind::Space, AxisBinding::Columns),
//! ]);
//! axes.levels.push(AxisLevel::new(vec![0.6, 0.5], vec![0.0, 0.0]));
//! let mut records = ScienceRecords::default();
//! records.calibration.push(c);
//! records.axes = Some(axes);
//! records.structured.push(StructuredMetadata::new(
//!     "dicom-json",
//!     StructuredFormat::Json,
//!     br#"{"00281054":{"vr":"LO","Value":["HU"]}}"#.to_vec(),
//! ));
//!
//! let mut meta = Metadata::default();
//! records.write_metadata(&info, &mut meta)?; // validated; adds the chunks
//! assert_eq!(ScienceRecords::from_metadata(&info, &meta)?, records);
//! # #[cfg(feature = "encode")]
//! # {
//! let file = ImiEncoderConfig::lossless().encode_samples(ImageRef::packed(&pixels, &info), &meta)?;
//!
//! let (info, meta) = imij::probe(&file)?;
//! let back = ScienceRecords::from_metadata(&info, &meta)?; // Corrupt if malformed
//! assert_eq!(back, records);
//! # }
//! # Ok::<(), imij::Error>(())
//! ```

use alloc::{string::String, vec::Vec};

use crate::{Chunk, Error, ImageInfo, Metadata, put_varint};

/// Calibration, nodata and units of each channel.
pub(crate) const CALB: [u8; 4] = *b"cALB";
/// Axes, per-level transforms, affine and pixel convention.
pub(crate) const AXES: [u8; 4] = *b"aXES";
/// Integer coordinates on the coordinate-bound axes.
pub(crate) const CORD: [u8; 4] = *b"cORD";
/// Structured metadata (schema id, format, packed bytes); repeatable.
pub(crate) const SMDT: [u8; 4] = *b"sMDT";
/// Categories: value to color, name and description, per channel.
pub(crate) const CATG: [u8; 4] = *b"cATG";
/// MIME type of the plane-set attachment that holds per-plane records.
pub(crate) const PLANE_SET_MIME: &str = "application/x.imij-science";
/// Name the builder gives that attachment (readers find it by MIME type).
pub(crate) const PLANE_SET_NAME: &str = "imij.science";

mod cbor;
mod wire;

pub use cbor::{CborEvent, CborLimits, CborReader};
use wire::{
    Parts, VERSION, encode_axes, encode_calibration, encode_categories, format_byte, put_str,
    validate_with,
};
pub use wire::{StructuredIdentity, structured_identity};
#[cfg(feature = "std")]
pub(crate) use wire::{check_strict_record, validate_plane_records};
pub(crate) use wire::{decode_plane, encode_plane_entries};

/// Exact integer sentinels in a version-2 `cALB` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegerNodata {
    Unsigned(Vec<u64>),
    Signed(Vec<i64>),
}

impl IntegerNodata {
    fn len(&self) -> usize {
        match self {
            Self::Unsigned(values) => values.len(),
            Self::Signed(values) => values.len(),
        }
    }
}

/// How the stored samples of one channel map to a physical quantity, and
/// which of them are missing. Stored samples are the decoded container values
/// (after channel maps), compared and scaled as binary64: exact for integer
/// samples of up to 32 bits and for floats, but 64-bit integer samples beyond
/// ±2^53 round, so a nodata value or range bound can also match neighbouring
/// stored values (a version 1 limitation).
///
/// `physical = slope * stored + offset` (FITS `BSCALE`/`BZERO`, CF
/// `scale_factor`/`add_offset`, DICOM Rescale Slope/Intercept, GDAL band
/// scale/offset). `nodata` and the valid range are in the stored domain, as
/// CF `_FillValue`/`valid_range`, FITS `BLANK` and GDAL nodata are.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ChannelCalibration {
    /// Finite and nonzero; 1 by default.
    pub slope: f64,
    /// Finite; 0 by default.
    pub offset: f64,
    /// Unit of the physical value, a UDUNITS-2 string (as CF and OME-NGFF use).
    pub unit: Option<String>,
    /// Name of the physical quantity: a CF `standard_name` when
    /// `quantity_is_standard_name`, else free text (DICOM Rescale Type `HU`).
    pub quantity: Option<String>,
    pub quantity_is_standard_name: bool,
    /// Stored values that mark a sample missing (binary64 equality, so 0.0
    /// also matches -0.0). Not NaN: use `nan_is_missing`. At most 16.
    pub nodata: Vec<f64>,
    /// Exact integer sentinels. When present, `nodata` must be empty. Use
    /// `is_missing_u64` or `is_missing_i64` for exact comparison.
    pub integer_nodata: Option<IntegerNodata>,
    /// Inclusive valid range of stored values; a non-NaN sample outside it
    /// is missing. Not NaN; `valid_min <= valid_max` when both are given.
    pub valid_min: Option<f64>,
    pub valid_max: Option<f64>,
    /// NaN samples are missing (FITS and most float conventions).
    pub nan_is_missing: bool,
    /// Plane (`PlaneId::index`) of this file's plane set whose zero samples
    /// mark missing samples, read at the level with this image's size.
    pub mask_plane: Option<u32>,
}

impl Default for ChannelCalibration {
    /// The identity: physical equals stored, nothing missing.
    fn default() -> Self {
        Self {
            slope: 1.0,
            offset: 0.0,
            unit: None,
            quantity: None,
            quantity_is_standard_name: false,
            nodata: Vec::new(),
            integer_nodata: None,
            valid_min: None,
            valid_max: None,
            nan_is_missing: false,
            mask_plane: None,
        }
    }
}

impl ChannelCalibration {
    /// The physical value of a stored sample: binary64 multiply, then add.
    /// Meaningless for missing samples.
    pub fn physical(&self, stored: f64) -> f64 {
        self.slope * stored + self.offset
    }

    /// Whether a stored value is missing by its value alone (NaN flag,
    /// nodata values, valid range); the mask plane is separate.
    pub fn is_missing(&self, stored: f64) -> bool {
        if stored.is_nan() {
            return self.nan_is_missing;
        }
        self.nodata.contains(&stored)
            || self.valid_min.is_some_and(|m| stored < m)
            || self.valid_max.is_some_and(|m| stored > m)
    }

    /// Checks exact unsigned sentinels and the binary64 calibration rules.
    pub fn is_missing_u64(&self, stored: u64) -> bool {
        matches!(&self.integer_nodata, Some(IntegerNodata::Unsigned(values)) if values.contains(&stored))
            || self.is_missing(stored as f64)
    }

    /// Checks exact signed sentinels and the binary64 calibration rules.
    pub fn is_missing_i64(&self, stored: i64) -> bool {
        matches!(&self.integer_nodata, Some(IntegerNodata::Signed(values)) if values.contains(&stored))
            || self.is_missing(stored as f64)
    }
}

/// The type of an axis, as OME-NGFF `axes[].type`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AxisKind {
    Space,
    Time,
    Channel,
    /// A custom type name; empty when unspecified.
    Other(String),
}

/// Which part of the stored data an axis runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AxisBinding {
    /// Indexed by the target's coordinates record (`cORD`): the c, z or t
    /// of one plane in a plane set, or of one image within a larger array.
    Coordinate,
    /// Image columns (x).
    Columns,
    /// Image rows (y).
    Rows,
    /// The interleaved channels of the image (single images only).
    Channels,
    /// The frame or page index of an `aNIM` file (single images only).
    Frames,
}

/// One axis: OME-NGFF `axes[]`, plus how it maps onto the stored data.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Axis {
    /// Nonempty and unique within the record.
    pub name: String,
    pub kind: AxisKind,
    pub binding: AxisBinding,
    /// UDUNITS-2 unit (OME-NGFF lists `micrometer`, `second`, ...).
    pub unit: Option<String>,
}

impl Axis {
    pub fn new(name: impl Into<String>, kind: AxisKind, binding: AxisBinding) -> Self {
        Self {
            name: name.into(),
            kind,
            binding,
            unit: None,
        }
    }
}

/// One resolution level's transform from array index to physical
/// coordinates, `u[i] = scale[i] * index[i] + translation[i]`: OME-NGFF
/// `datasets[].coordinateTransformations` (one scale, then one translation).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AxisLevel {
    /// One finite, nonzero value per axis.
    pub scale: Vec<f64>,
    /// One finite value per axis.
    pub translation: Vec<f64>,
}

impl AxisLevel {
    pub fn new(scale: Vec<f64>, translation: Vec<f64>) -> Self {
        Self { scale, translation }
    }
}

/// Axes and coordinates of the stored data (`aXES`).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Axes {
    /// 2 to 32 axes; exactly one bound to columns and one to rows.
    pub axes: Vec<Axis>,
    /// Level 0 is the full-resolution image (a plane's level 0); at most 64.
    pub levels: Vec<AxisLevel>,
    /// Optional `N x (N + 1)` row-major matrix `A` applied after the level
    /// transform: `physical = A * [u; 1]` (OME-NGFF RFC-5 affine layout).
    pub affine: Option<Vec<f64>>,
    /// Pixel `i` covers `[i, i + 1)` in index space (GeoTIFF PixelIsArea,
    /// GDAL). When false, index `i` is the pixel's center and the pixel
    /// covers `[i - 0.5, i + 0.5)` (OME-NGFF RFC-5, DICOM, PixelIsPoint).
    pub pixel_is_area: bool,
    /// How lower levels were made, OME-NGFF `multiscales[].type`
    /// (e.g. `gaussian`).
    pub downsampling: Option<String>,
}

impl Axes {
    pub fn new(axes: Vec<Axis>) -> Self {
        Self {
            axes,
            levels: Vec::new(),
            affine: None,
            pixel_is_area: false,
            downsampling: None,
        }
    }

    /// Physical coordinates of a (continuous) index at `level`, in axis
    /// order: the level's scale and translation, then the affine. `None`
    /// when the level is not declared or `index` has the wrong length.
    pub fn to_physical(&self, level: usize, index: &[f64]) -> Option<Vec<f64>> {
        let l = self.levels.get(level)?;
        let n = self.axes.len();
        if index.len() != n || l.scale.len() != n || l.translation.len() != n {
            return None;
        }
        let u: Vec<f64> = (0..n)
            .map(|i| l.scale[i] * index[i] + l.translation[i])
            .collect();
        let Some(a) = &self.affine else {
            return Some(u);
        };
        if a.len() != n * (n + 1) {
            return None;
        }
        Some(
            a.chunks_exact(n + 1)
                .map(|row| (0..n).fold(row[n], |acc, j| acc + row[j] * u[j]))
                .collect(),
        )
    }

    fn coordinate_axes(&self) -> usize {
        self.axes
            .iter()
            .filter(|a| a.binding == AxisBinding::Coordinate)
            .count()
    }
}

/// Encoding of a structured metadata record's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StructuredFormat {
    /// UTF-8 text whose syntax the schema defines (FITS cards, WKT2, XML).
    Text,
    /// UTF-8 JSON (RFC 8259).
    Json,
    /// CBOR (RFC 8949).
    Cbor,
}

/// Domain metadata kept verbatim (`sMDT`), for example an OME-NGFF `ome`
/// object, CF attributes, a FITS header, DICOM JSON or a WKT2 CRS.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StructuredMetadata {
    /// 1 to 64 printable ASCII bytes without spaces (`ome-ngff/0.5`,
    /// `cf-1.12`, `fits-header`, `dicom-json`, `wkt2`; SPEC.md 1.5).
    pub schema: String,
    /// Optional instance key, 1 to 64 printable ASCII bytes without spaces.
    pub instance_key: Option<String>,
    pub format: StructuredFormat,
    /// At most 16 MiB; UTF-8 for text and JSON.
    pub data: Vec<u8>,
}

impl StructuredMetadata {
    pub fn new(schema: impl Into<String>, format: StructuredFormat, data: Vec<u8>) -> Self {
        Self {
            schema: schema.into(),
            instance_key: None,
            format,
            data,
        }
    }

    /// Names this instance within its schema and image or sequence item.
    pub fn with_instance_key(mut self, key: impl Into<String>) -> Self {
        self.instance_key = Some(key.into());
        self
    }

    /// Inspect a CBOR instance using the bounded deterministic CBOR profile.
    pub fn cbor_events(&self, limits: CborLimits) -> Result<CborReader<'_>, Error> {
        if self.format != StructuredFormat::Cbor {
            return Err(Error::InvalidInput("structured metadata is not CBOR"));
        }
        CborReader::new(&self.data, limits)
    }
}

/// One category of a categorical channel (`cATG`): a label image, a
/// land-cover class map, a segmentation mask or a palette image keeps its
/// index values as integer samples, and this says what each value means.
/// OME-NGFF `image-label` colors, GDAL color tables and category names, the
/// TIFF ColorMap, HDF5 enumerations and CF `flag_values`/`flag_meanings`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Category {
    /// The stored sample value of this category. Integer samples match it
    /// exactly as integers; float samples match when they equal its binary64
    /// rounding (to nearest, ties to even), which is exact within ±2^53.
    /// Beyond that, distinct values can round alike and share samples
    /// (SPEC.md 1.5), so keep float-channel categories within ±2^53.
    pub value: i64,
    /// Display color R, G, B, A with 16-bit components (alpha 0 is
    /// transparent, 65535 opaque; straight alpha). An 8-bit component `v`
    /// is `v * 257`, as in the TIFF ColorMap; the record stores 8 bits when
    /// every component of every color is such a multiple.
    pub color: Option<[u16; 4]>,
    /// Short name, 1 to 256 bytes (CF `flag_meanings` word, GDAL category
    /// name, HDF5 enum member name).
    pub name: Option<String>,
    /// Longer description, 1 to 1024 bytes.
    pub description: Option<String>,
}

impl Category {
    pub fn new(value: i64) -> Self {
        Self {
            value,
            color: None,
            name: None,
            description: None,
        }
    }
}

/// The science records of one image or plane. Empty fields write nothing.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ScienceRecords {
    /// One entry per channel in memory order (planes: exactly one), or empty.
    pub calibration: Vec<ChannelCalibration>,
    pub axes: Option<Axes>,
    /// One index per [`AxisBinding::Coordinate`] axis of `axes`, in axis
    /// order (for a plane: of the plane's effective axes), or empty.
    pub coordinates: Vec<i64>,
    /// Distinct `(schema, instance_key)` pairs, at most 64.
    pub structured: Vec<StructuredMetadata>,
    /// Categories of each channel in memory order (planes: exactly one
    /// list), or empty. A channel's list is empty when it is not
    /// categorical; values strictly increase; at most 65,536 per channel.
    pub categories: Vec<Vec<Category>>,
}

impl ScienceRecords {
    pub fn is_empty(&self) -> bool {
        self.calibration.is_empty()
            && self.axes.is_none()
            && self.coordinates.is_empty()
            && self.structured.is_empty()
            && self.categories.is_empty()
    }

    /// The records of a single image (or of every frame or page of an
    /// animation or page file) from its decoded metadata. Records of an
    /// unknown version or structured format are skipped; malformed or
    /// repeated records are [`Error::Corrupt`].
    pub fn from_metadata(info: &ImageInfo, meta: &Metadata) -> Result<Self, Error> {
        let target = Target::image(info);
        let mut parts = Parts::default();
        for c in &meta.chunks {
            parts.add(c.tag, &c.data)?;
        }
        parts.decode(&target)
    }

    /// Replaces the science chunks in `meta` with these records, validated
    /// for an image described by `info`; empty records leave no chunks.
    /// The container writes them before `DATA`.
    pub fn write_metadata(&self, info: &ImageInfo, meta: &mut Metadata) -> Result<(), Error> {
        let chunks = self.encode(&Target::image(info))?;
        meta.chunks.retain(|c| !is_science_tag(&c.tag));
        meta.chunks
            .try_reserve(chunks.len())
            .map_err(|_| Error::TooLarge)?;
        for (tag, data) in chunks {
            meta.chunks.push(Chunk { tag, data });
        }
        Ok(())
    }

    /// Validates and serializes the records for `target` as (tag, payload).
    pub(crate) fn encode(&self, target: &Target) -> Result<Vec<([u8; 4], Vec<u8>)>, Error> {
        self.encode_for(target, self.axes.as_ref())
    }

    /// [`Self::encode`] with the axes the coordinates refer to (a plane may
    /// inherit the plane set's default axes).
    pub(crate) fn encode_for(
        &self,
        target: &Target,
        axes: Option<&Axes>,
    ) -> Result<Vec<([u8; 4], Vec<u8>)>, Error> {
        validate_with(self, target, axes).map_err(|e| match e {
            Error::Corrupt(s) => Error::InvalidInput(s),
            e => e,
        })?;
        let mut out = Vec::new();
        if !self.calibration.is_empty() {
            out.push((CALB, encode_calibration(&self.calibration)));
        }
        if let Some(a) = &self.axes {
            out.push((AXES, encode_axes(a)));
        }
        if !self.coordinates.is_empty() {
            let mut b = alloc::vec![VERSION];
            put_varint(&mut b, self.coordinates.len() as u64);
            for &v in &self.coordinates {
                put_varint(&mut b, ((v << 1) ^ (v >> 63)) as u64);
            }
            out.push((CORD, b));
        }
        for s in &self.structured {
            let version = if s.instance_key.is_some() { 2 } else { VERSION };
            let mut b = alloc::vec![version, format_byte(s.format)];
            put_str(&mut b, &s.schema);
            if let Some(key) = &s.instance_key {
                put_str(&mut b, key);
            }
            b.extend_from_slice(&crate::metadata_pack::pack(&s.data)?);
            out.push((SMDT, b));
        }
        if !self.categories.is_empty() {
            out.push((CATG, encode_categories(&self.categories)));
        }
        Ok(out)
    }
}

pub(crate) fn is_science_tag(tag: &[u8; 4]) -> bool {
    matches!(tag, &CALB | &AXES | &CORD | &SMDT | &CATG)
}

/// What a record set describes, for validation.
pub(crate) struct Target {
    /// Channels of the image (planes: 1).
    channels: usize,
    sample: Option<crate::SampleType>,
    /// A single image (or frames): channel and frame bindings are allowed.
    image: bool,
    /// A plane set's default records: no coordinates.
    default: bool,
    /// Plane count of the plane set, when known, for mask references.
    planes: Option<u32>,
}

impl Target {
    pub(crate) fn image(info: &ImageInfo) -> Self {
        Self {
            channels: info.channels.count(),
            sample: Some(info.sample),
            image: true,
            default: false,
            planes: None,
        }
    }
    pub(crate) fn plane(planes: u32, default: bool, sample: Option<crate::SampleType>) -> Self {
        Self {
            channels: 1,
            sample,
            image: false,
            default,
            planes: Some(planes),
        }
    }
    /// A plane target whose plane count is not final yet (builder checks
    /// before more planes are added; mask references are checked later).
    #[cfg(feature = "encode")]
    pub(crate) fn plane_pending(default: bool, sample: Option<crate::SampleType>) -> Self {
        Self {
            channels: 1,
            sample,
            image: false,
            default,
            planes: None,
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, SampleType};
    use alloc::vec;

    #[test]
    fn identity_entry_is_four_bytes() {
        let mut rec = ScienceRecords::default();
        rec.calibration.push(ChannelCalibration::default());
        let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U16);
        let chunks = rec.encode(&Target::image(&info)).unwrap();
        assert_eq!(chunks, vec![(CALB, vec![1, 1, 0, 0])]);
    }
}
