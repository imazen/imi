//! Strict V3/V4 validation always reconstructs every recognized image payload.
//! Input remains positional; decoded reference rasters use caller-owned scratch.
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::parse::{chunks, header, payload::HeadInput};
use crate::planes::ReadAt;
use crate::traversal::{self, Cursor, RangeSource, Scan, ScanLimits};
use crate::{ByteReader, Digest, Error, FrameKind, HashAlgorithm, ImageInfo};
use alloc::vec::Vec;
use std::io::{Read, Seek, SeekFrom, Write};

/// Resource policy, separate from format validity. Exceeding any bound fails
/// validation; no image is skipped and no partial success is returned.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ValidationLimits {
    pub max_file_bytes: u64,
    pub max_header_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_chunks: usize,
    pub max_units: usize,
    pub max_depth: usize,
    pub max_images: u64,
    pub max_decoded_pixels: u64,
    pub max_scratch_bytes: u64,
    pub decode: DecodePolicy,
    pub planes: crate::planes::PlaneSetLimits,
}
impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1 << 40,
            max_header_bytes: 1 << 20,
            max_metadata_bytes: crate::decode_resources::DEFAULT_METADATA_BYTES,
            max_chunks: 1_000_000,
            max_units: 1_000_000,
            max_depth: 16,
            max_images: 1_000_000,
            max_decoded_pixels: 1 << 40,
            max_scratch_bytes: 1 << 40,
            decode: DecodePolicy::default()
                .with_output_limits(crate::Limits {
                    max_pixels: 1 << 40,
                    max_bytes: 1 << 44,
                    verify_checksum: true,
                })
                .with_max_input_bytes(1 << 44)
                .with_max_work_units(1 << 50),
            planes: crate::planes::PlaneSetLimits::default()
                .with_file_bytes(1 << 40)
                .with_plane_pixels(1 << 40),
        }
    }
}
/// Returned only after complete reconstruction and verification. Counts do not
/// imply that an absent checksum was present. Opaque ancillary extensions and
/// attachments have no pixel semantics assigned by this version of the format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationReport {
    pub images: u64,
    pub decoded_pixels: u64,
    pub file_checksums: u64,
    pub pixel_checksums: u64,
    pub reference_checksums: u64,
    pub opaque_chunks: u64,
    pub scratch_bytes: u64,
}

/// Validate a complete V3/V4 file, including all known nested image payloads.
///
/// Every successful call decodes all pixels and verifies every declared file,
/// pixel, reference and V4 scoped check, and each recognized float guarantee.
/// Unsupported check suites, unsupported image coding, malformed known records,
/// stale indexes and legacy nested files fail.
/// There is no structure-only mode.
///
/// The source must remain immutable. Scratch is appended to its existing end;
/// independent images need no raster scratch. Use a disk file for large reference
/// rasters or animation canvases. Its cursor changes and appended
/// bytes remain on both success and failure. Input and scratch must not alias.
/// Bounds limit planned decoding storage, indexes and disk use, not process RSS.
pub fn validate<R: ReadAt, S: Read + Write + Seek>(
    source: &mut R,
    scratch: &mut S,
    limits: &ValidationLimits,
) -> Result<ValidationReport, Error> {
    validate_v3(source, scratch, limits)
}

/// Compatibility name for [`validate`]; accepts both current wire grammars.
pub fn validate_v3<R: ReadAt, S: Read + Write + Seek>(
    source: &mut R,
    scratch: &mut S,
    limits: &ValidationLimits,
) -> Result<ValidationReport, Error> {
    let start = scratch.seek(SeekFrom::End(0)).map_err(|_| Error::Read)?;
    let mut limits = *limits;
    let mut output = limits.decode.output_limits();
    output.verify_checksum = true;
    limits.decode = limits
        .decode
        .with_output_limits(output)
        .with_scoped_pixel_checks(true, true);
    let mut validator = Validator {
        scratch,
        start,
        end: start,
        limits,
        session: DecodeSession::new(limits.decode),
        report: ValidationReport::default(),
    };
    validator.image(source, 0, ImageContext::default())?;
    validator.report.scratch_bytes = validator.end - start;
    Ok(validator.report)
}

#[derive(Clone)]
struct Raster {
    info: ImageInfo,
    at: Option<u64>,
}
impl Raster {
    fn offset(&self) -> Result<u64, Error> {
        self.at
            .ok_or(Error::Corrupt("missing retained reference raster"))
    }
}
#[derive(Default)]
struct ImageContext<'a> {
    reference: Option<Reference>,
    inherited: Option<&'a ImageInfo>,
    single: bool,
    retain: bool,
    checksum: Option<(usize, Digest)>,
}
#[derive(Clone)]
struct Reference {
    raster: Raster,
    x: u32,
    y: u32,
}
struct Validator<'a, S> {
    scratch: &'a mut S,
    start: u64,
    end: u64,
    limits: ValidationLimits,
    session: DecodeSession,
    report: ValidationReport,
}
struct RasterRows<'a, S> {
    scratch: &'a mut S,
    output: Raster,
    reference: Option<Reference>,
    next: u32,
    checks: [Option<crate::pixelsum::RowCheck>; 2],
    float_guarantees: [crate::FloatGuarantees; 4],
}
impl<S: Read + Write + Seek> crate::read::input::Rows for RasterRows<'_, S> {
    fn has_reference(&self) -> bool {
        self.reference.is_some()
    }
    async fn reference(&mut self, info: &ImageInfo, y: u32, out: &mut [u8]) -> Result<(), Error> {
        let r = self
            .reference
            .as_ref()
            .ok_or(Error::Corrupt("missing reference"))?;
        let offset = r.raster.offset()?
            + u64::from(y + r.y) * r.raster.info.row_bytes() as u64
            + u64::from(r.x) * info.bytes_per_pixel() as u64;
        self.scratch
            .seek(SeekFrom::Start(offset))
            .map_err(|_| Error::Read)?;
        self.scratch.read_exact(out).map_err(|_| Error::Read)
    }
    async fn row(&mut self, info: &ImageInfo, y: u32, pixels: &[u8]) -> Result<(), Error> {
        if y != self.next || info != &self.output.info || pixels.len() != info.row_bytes() {
            return Err(Error::Corrupt("decoded raster rows differ"));
        }
        if !crate::float_guarantees::row_matches(pixels, info, self.float_guarantees) {
            return Err(Error::Corrupt("V4 float guarantee violated"));
        }
        for check in self.checks.iter_mut().flatten() {
            check.row(pixels)?;
        }
        if let Some(at) = self.output.at {
            self.scratch
                .seek(SeekFrom::Start(at + u64::from(y) * pixels.len() as u64))
                .map_err(|_| Error::Read)?;
            self.scratch.write_all(pixels).map_err(|_| Error::Read)?;
        }
        self.next += 1;
        Ok(())
    }
}

struct Primary<'a> {
    source: &'a mut dyn ReadAt,
    header: std::io::Cursor<Vec<u8>>,
    ranges: Vec<core::ops::Range<u64>>,
    index: usize,
}
impl Read for Primary<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.header.read(out)?;
        if n != 0 || out.is_empty() {
            return Ok(n);
        }
        while let Some(range) = self.ranges.get_mut(self.index) {
            if range.is_empty() {
                self.index += 1;
                continue;
            }
            let n = (range.end - range.start).min(out.len() as u64) as usize;
            self.source
                .read_exact_at(range.start, &mut out[..n])
                .map_err(std::io::Error::other)?;
            range.start += n as u64;
            return Ok(n);
        }
        Ok(0)
    }
}

impl<S: Read + Write + Seek> Validator<'_, S> {
    fn allocate(&mut self, info: ImageInfo) -> Result<Raster, Error> {
        let bytes = (info.row_bytes() as u64)
            .checked_mul(u64::from(info.height))
            .ok_or(Error::TooLarge)?;
        let end = self.end.checked_add(bytes).ok_or(Error::TooLarge)?;
        if end - self.start > self.limits.max_scratch_bytes {
            return Err(Error::TooLarge);
        }
        let raster = Raster {
            info,
            at: Some(self.end),
        };
        self.end = end;
        Ok(raster)
    }
    fn digest(
        &mut self,
        raster: &Raster,
        ordinal: u64,
        algo: HashAlgorithm,
    ) -> Result<Digest, Error> {
        let rb = raster.info.row_bytes();
        let mut row = Vec::new();
        row.try_reserve_exact(rb).map_err(|_| Error::TooLarge)?;
        row.resize(rb, 0);
        let mut hash = crate::pixelsum::Raster::new(algo, &raster.info, ordinal)?;
        self.scratch
            .seek(SeekFrom::Start(raster.offset()?))
            .map_err(|_| Error::Read)?;
        for _ in 0..raster.info.height {
            self.scratch.read_exact(&mut row).map_err(|_| Error::Read)?;
            hash.row(&row)?;
        }
        hash.finish()
    }
    fn check_pixel(
        &mut self,
        table: crate::pixelsum::Table<'_>,
        raster: &Raster,
        ordinal: usize,
    ) -> Result<(), Error> {
        if let Some(expected) = table.expected(ordinal)? {
            if self.digest(raster, ordinal as u64, expected.algorithm())? != expected {
                return Err(crate::pixelsum::MISMATCH);
            }
            self.report.pixel_checksums += 1;
        }
        Ok(())
    }
    fn image(
        &mut self,
        source: &mut dyn ReadAt,
        depth: usize,
        context: ImageContext<'_>,
    ) -> Result<Raster, Error> {
        let ImageContext {
            reference,
            inherited,
            single,
            retain,
            checksum,
        } = context;
        if depth > self.limits.max_depth || self.report.images >= self.limits.max_images {
            return Err(Error::TooLarge);
        }
        let scan = traversal::scan(
            source,
            ScanLimits {
                file: self.limits.max_file_bytes,
                head: self.limits.max_header_bytes,
                metadata: self.limits.max_metadata_bytes,
                chunks: self.limits.max_chunks,
                units: self.limits.max_units,
                decode: self.limits.decode,
            },
        )?;
        if !matches!(scan.header.header.version, 3 | 4) {
            return Err(Error::Unsupported("strict validation requires v3/v4"));
        }
        if single && (scan.chunk(&chunks::ANIM).is_some() || scan.chunk(&chunks::FRAM).is_some()) {
            return Err(Error::Corrupt("nested sequence item contains frames"));
        }
        self.report.file_checksums += u64::from(scan.checked_file_checksum);
        let mut info = scan.header.header.info.clone();
        if !scan.native
            && scan.header.header.lossy_color.is_some()
            && let Some(parent) = inherited
        {
            info.cicp = parent.cicp;
            info.alpha_mode = parent.alpha_mode;
        }
        let pixels = u64::from(info.width) * u64::from(info.height);
        self.report.decoded_pixels = self
            .report
            .decoded_pixels
            .checked_add(pixels)
            .ok_or(Error::TooLarge)?;
        if self.report.decoded_pixels > self.limits.max_decoded_pixels {
            return Err(Error::TooLarge);
        }
        self.report.images += 1;
        let pixel_bytes = if let Some(c) = scan.chunk(&crate::pixelsum::PSUM) {
            scan.bytes(source, c, self.limits.max_metadata_bytes)?
        } else {
            Vec::new()
        };
        let pixel_table = if pixel_bytes.is_empty() {
            crate::pixelsum::Table::Absent
        } else {
            crate::pixelsum::parse(&pixel_bytes)?
        };
        let anim = if let Some(c) = scan.chunk(&chunks::ANIM) {
            Some(scan.bytes(source, c, self.limits.max_header_bytes)?)
        } else {
            None
        };
        let frames = scan.chunks.iter().filter(|c| c.tag == chunks::FRAM).count();
        let (kind, _, count, _) = crate::frames::sequence_control_parts(
            anim.as_deref(),
            frames,
            scan.header.declared,
            true,
        )?;
        pixel_table.check_count(count)?;
        let retained =
            retention_plan(source, &scan, anim.as_deref(), self.limits.max_header_bytes)?;
        let raster = if retain || retained[0] {
            self.allocate(info.clone())?
        } else {
            Raster {
                info: info.clone(),
                at: None,
            }
        };
        let own_check = crate::pixelsum::RowCheck::new(pixel_table, &info)?;
        let parent_check = checksum
            .map(|(ordinal, digest)| {
                crate::pixelsum::RowCheck::for_digest(digest, &info, ordinal as u64)
            })
            .transpose()?;
        let check_count = u64::from(own_check.is_some()) + u64::from(parent_check.is_some());
        let head = if inherited.is_none() || scan.native || scan.header.header.lossy_color.is_none()
        {
            let head = crate::container::with_extension(
                scan.header_bytes.clone(),
                crate::checksum::EXT_CHECKSUM,
                None,
            )?;
            crate::container::with_extension(head, header::EXT_LOCATOR, None)?
        } else {
            let mut head = Vec::new();
            crate::container::write_group_head_features(
                &mut head,
                &info,
                &crate::Metadata {
                    orientation: scan.header.header.orientation,
                    ..Default::default()
                },
                scan.header.header.coding,
                scan.header.header.lossy_color,
                scan.header.header.decode_speed,
                scan.header.declared,
                scan.header.header.version,
            )?;
            head
        };
        let mut ranges = Vec::new();
        for tag in [chunks::DATA, chunks::UIDX] {
            if let Some(c) = scan.chunk(&tag) {
                ranges.push(c.start..c.body.end);
            }
        }
        let input = Primary {
            source,
            header: std::io::Cursor::new(head),
            ranges,
            index: 0,
        };
        let float_guarantees = scan.header.header.float_guarantees;
        if float_guarantees != crate::float_guarantees::EMPTY {
            self.session
                .adapter_work(u128::from(pixels) * info.bytes_per_pixel() as u128)?;
        }
        let mut rows = RasterRows {
            scratch: self.scratch,
            output: raster.clone(),
            reference,
            next: 0,
            checks: [own_check, parent_check],
            float_guarantees,
        };
        self.session.decode_reader_rows(input, &mut rows)?;
        if rows.next != info.height {
            return Err(Error::Corrupt("missing decoded rows"));
        }
        self.report.pixel_checksums += check_count;
        self.nested(
            source,
            &scan,
            depth,
            &raster,
            kind,
            anim.as_deref(),
            pixel_table,
            &retained,
        )?;
        Ok(raster)
    }
    #[allow(clippy::too_many_arguments)]
    fn nested(
        &mut self,
        source: &mut dyn ReadAt,
        scan: &Scan,
        depth: usize,
        primary: &Raster,
        kind: FrameKind,
        anim: Option<&[u8]>,
        table: crate::pixelsum::Table<'_>,
        retained: &[bool],
    ) -> Result<(), Error> {
        let unified = anim.is_some_and(|a| a.first() == Some(&4));
        let mut images = Vec::new();
        images.try_reserve(1).map_err(|_| Error::TooLarge)?;
        images.push(primary.clone());
        let mut metadata = crate::Metadata::default();
        let mut metadata_bytes = 0usize;
        for chunk in &scan.chunks {
            match chunk.tag {
                chunks::FRAM => {
                    let (control_bytes, nested) =
                        frame_prefix(source, chunk, self.limits.max_header_bytes)?;
                    let control =
                        crate::frames::frame_control(&control_bytes, scan.header.header.version)?;
                    let ordinal = images.len();
                    if control.duration.den == 0 {
                        return Err(Error::Corrupt("zero frame duration denominator"));
                    }
                    let previous = control.present & 1 != 0;
                    let empty = control.present & 2 != 0;
                    if !unified && control.reference.is_some() {
                        return Err(Error::Corrupt("reference outside unified sequence"));
                    }
                    if unified && (control.x0 != 0 || control.y0 != 0 || previous || empty) {
                        return Err(Error::Corrupt("unified item controls"));
                    }
                    let mut checked = false;
                    let image = if empty {
                        if kind != FrameKind::Animation
                            || !previous
                            || !nested.is_empty()
                            || control.x0 > primary.info.width
                            || control.y0 > primary.info.height
                        {
                            return Err(Error::Corrupt("invalid empty frame"));
                        }
                        images.last().unwrap().clone()
                    } else {
                        let mut child = RangeSource {
                            source,
                            range: nested,
                        };
                        let hp = read_image_header(&mut child, self.limits.max_header_bytes)?;
                        if !scan.header.declared.covers(hp.declared) {
                            return Err(Error::Corrupt("undeclared nested frame feature"));
                        }
                        let child_info = &hp.header.info;
                        if unified && kind == FrameKind::Animation && child_info != &primary.info {
                            return Err(Error::Corrupt("animation canvas format changes"));
                        }
                        let reference = if unified {
                            if let Some((index, expected)) = control.reference {
                                let earlier = images
                                    .get(usize::try_from(index).map_err(|_| Error::TooLarge)?)
                                    .ok_or(Error::Corrupt("reference is not an earlier image"))?;
                                if &earlier.info != child_info {
                                    return Err(Error::Corrupt(
                                        "reference pixel description differs",
                                    ));
                                }
                                if self.digest(earlier, index, expected.algorithm())? != expected {
                                    return Err(Error::Corrupt("reference checksum mismatch"));
                                }
                                self.report.reference_checksums += 1;
                                Some(Reference {
                                    raster: earlier.clone(),
                                    x: 0,
                                    y: 0,
                                })
                            } else {
                                None
                            }
                        } else {
                            if kind != FrameKind::MixedPages
                                && (child_info.channels != primary.info.channels
                                    || child_info.sample != primary.info.sample
                                    || (0..child_info.channels.count()).any(|c| {
                                        child_info.channel_bits(c) != primary.info.channel_bits(c)
                                    }))
                            {
                                return Err(Error::Corrupt(
                                    "frame pixel format differs from canvas",
                                ));
                            }
                            if kind == FrameKind::Animation {
                                let fits = control
                                    .x0
                                    .checked_add(child_info.width)
                                    .is_some_and(|v| v <= primary.info.width)
                                    && control
                                        .y0
                                        .checked_add(child_info.height)
                                        .is_some_and(|v| v <= primary.info.height);
                                let full = control.x0 == 0
                                    && control.y0 == 0
                                    && child_info.width == primary.info.width
                                    && child_info.height == primary.info.height;
                                if !fits || (!previous && !full) {
                                    return Err(Error::Corrupt("frame rectangle"));
                                }
                            } else if control.x0 != 0 || control.y0 != 0 || previous {
                                return Err(Error::Corrupt("page with rectangle"));
                            }
                            previous.then(|| Reference {
                                raster: images.last().unwrap().clone(),
                                x: control.x0,
                                y: control.y0,
                            })
                        };
                        let inherited =
                            (!unified && kind != FrameKind::MixedPages).then_some(&primary.info);
                        let compose = !unified
                            && kind == FrameKind::Animation
                            && (control.x0 != 0
                                || control.y0 != 0
                                || child_info.width != primary.info.width
                                || child_info.height != primary.info.height);
                        let checksum = if compose {
                            None
                        } else {
                            table.expected(ordinal)?.map(|digest| (ordinal, digest))
                        };
                        let decoded = self.image(
                            &mut child,
                            depth + 1,
                            ImageContext {
                                reference,
                                inherited,
                                single: true,
                                retain: retained[ordinal] || compose,
                                checksum,
                            },
                        )?;
                        checked = !compose;
                        if compose {
                            self.composite(
                                images.last().unwrap(),
                                &decoded,
                                control.x0,
                                control.y0,
                            )?
                        } else {
                            decoded
                        }
                    };
                    if !checked {
                        self.check_pixel(table, &image, ordinal)?;
                    }
                    images.try_reserve(1).map_err(|_| Error::TooLarge)?;
                    images.push(image);
                }
                chunks::PRVW => {
                    let mut child = RangeSource {
                        source,
                        range: chunk.body.clone(),
                    };
                    self.image(&mut child, depth + 1, ImageContext::default())?;
                }
                chunks::GMAP => {
                    let range = gain_image(source, chunk, self.limits.max_metadata_bytes)?;
                    let mut child = RangeSource { source, range };
                    self.image(&mut child, depth + 1, ImageContext::default())?;
                }
                chunks::DATA
                | chunks::UIDX
                | chunks::CSUM
                | chunks::ANIM
                | crate::pixelsum::PSUM => {}
                tag if chunks::is_plane_chunk(&tag) => {}
                tag if tag == *b"cORg"
                    || crate::item_metadata::is_tag(&tag)
                    || crate::science::is_science_tag(&tag)
                    || matches!(
                        tag,
                        chunks::ICCP
                            | chunks::EXIF
                            | chunks::XMP
                            | chunks::MDCV
                            | chunks::CLLI
                            | chunks::DWHI
                            | chunks::MCMP
                    ) =>
                {
                    let bytes = scan.bytes(
                        source,
                        chunk,
                        self.limits
                            .max_metadata_bytes
                            .saturating_sub(metadata_bytes),
                    )?;
                    metadata_bytes = metadata_bytes
                        .checked_add(bytes.len())
                        .ok_or(Error::TooLarge)?;
                    let expanded = if tag == chunks::MCMP {
                        crate::metadata_pack::unpacked_len(&bytes[4..])?
                    } else {
                        crate::science::check_strict_record(tag, &bytes)?
                    };
                    metadata_bytes = metadata_bytes
                        .checked_add(expanded)
                        .ok_or(Error::TooLarge)?;
                    if metadata_bytes > self.limits.max_metadata_bytes {
                        return Err(Error::TooLarge);
                    }
                    crate::container::keep_metadata(&mut metadata, tag, &bytes)?;
                }
                _ => self.report.opaque_chunks += 1,
            }
        }
        crate::science::ScienceRecords::from_metadata(&primary.info, &metadata)?;
        crate::item_metadata::ItemMetadata::from_metadata_with_byte_limit(
            &metadata,
            self.limits.max_metadata_bytes,
        )?;
        metadata.color_origin()?;
        if scan.chunks.iter().any(|c| chunks::is_plane_chunk(&c.tag)) {
            crate::planes::validate_all_tiles(
                source,
                self.limits.planes,
                &scan.chunks,
                self.limits.max_metadata_bytes,
                |bytes| {
                    self.image(
                        &mut crate::planes::SliceReader::new(bytes),
                        depth + 1,
                        ImageContext {
                            single: true,
                            ..Default::default()
                        },
                    )
                    .map(|_| ())
                },
            )?;
        }
        Ok(())
    }
    fn composite(
        &mut self,
        previous: &Raster,
        update: &Raster,
        x: u32,
        y: u32,
    ) -> Result<Raster, Error> {
        if x == 0
            && y == 0
            && update.info.width == previous.info.width
            && update.info.height == previous.info.height
        {
            return Ok(Raster {
                info: previous.info.clone(),
                at: update.at,
            });
        }
        let out = self.allocate(previous.info.clone())?;
        let rb = previous.info.row_bytes();
        let mut row = Vec::new();
        row.try_reserve_exact(rb).map_err(|_| Error::TooLarge)?;
        row.resize(rb, 0);
        for dy in 0..previous.info.height {
            self.scratch
                .seek(SeekFrom::Start(
                    previous.offset()? + u64::from(dy) * rb as u64,
                ))
                .map_err(|_| Error::Read)?;
            self.scratch.read_exact(&mut row).map_err(|_| Error::Read)?;
            if dy >= y && dy - y < update.info.height {
                let start = x as usize * previous.info.bytes_per_pixel();
                self.scratch
                    .seek(SeekFrom::Start(
                        update.offset()? + u64::from(dy - y) * update.info.row_bytes() as u64,
                    ))
                    .map_err(|_| Error::Read)?;
                self.scratch
                    .read_exact(&mut row[start..][..update.info.row_bytes()])
                    .map_err(|_| Error::Read)?;
            }
            self.scratch
                .seek(SeekFrom::Start(out.offset()? + u64::from(dy) * rb as u64))
                .map_err(|_| Error::Read)?;
            self.scratch.write_all(&row).map_err(|_| Error::Read)?;
        }
        Ok(out)
    }
}

fn retention_plan(
    source: &mut dyn ReadAt,
    scan: &Scan,
    anim: Option<&[u8]>,
    cap: usize,
) -> Result<Vec<bool>, Error> {
    let count = 1 + scan.chunks.iter().filter(|c| c.tag == chunks::FRAM).count();
    let mut retained = Vec::new();
    retained
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    retained.resize(count, false);
    let unified = anim.is_some_and(|a| a.first() == Some(&4));
    for (i, chunk) in scan
        .chunks
        .iter()
        .filter(|c| c.tag == chunks::FRAM)
        .enumerate()
    {
        let ordinal = i + 1;
        let (bytes, _) = frame_prefix(source, chunk, cap)?;
        let control = crate::frames::frame_control(&bytes, scan.header.header.version)?;
        if unified {
            if let Some((index, _)) = control.reference {
                let index = usize::try_from(index).map_err(|_| Error::TooLarge)?;
                if index >= ordinal {
                    return Err(Error::Corrupt("reference is not an earlier image"));
                }
                retained[index] = true;
            }
        } else if control.present & 1 != 0 {
            retained[ordinal - 1] = true;
        }
    }
    Ok(retained)
}

fn read_image_header(source: &mut dyn ReadAt, cap: usize) -> Result<header::ParsedHeader, Error> {
    let len = source.len()?;
    let mut r = Cursor::new(source, len);
    if r.bytes(8)? != header::SIGNATURE {
        return Err(Error::NotImij);
    }
    let n = r.varint()?;
    let end = r.pos.checked_add(n).ok_or(Error::TooLarge)?;
    r.pos = 0;
    header::read_header_sized(&mut ByteReader::new(&r.vector(end, cap)?))
}
fn frame_prefix(
    source: &mut dyn ReadAt,
    chunk: &traversal::Chunk,
    cap: usize,
) -> Result<(Vec<u8>, core::ops::Range<u64>), Error> {
    let mut r = Cursor::new(source, chunk.body.end);
    r.pos = chunk.body.start;
    r.limit_head(cap as u64);
    for _ in 0..4 {
        r.varint()?;
    }
    let flags = r.varint()?;
    if flags & 4 != 0 {
        let n = r.varint()?;
        r.vector(n, cap)?;
    }
    let end = r.pos;
    r.pos = chunk.body.start;
    Ok((r.vector(end - r.pos, cap)?, end..chunk.body.end))
}
fn gain_image(
    source: &mut dyn ReadAt,
    chunk: &traversal::Chunk,
    cap: usize,
) -> Result<core::ops::Range<u64>, Error> {
    let mut r = Cursor::new(source, chunk.body.end);
    r.pos = chunk.body.start;
    r.limit_head(cap as u64);
    if r.u8()? != 0 {
        return Err(Error::Unsupported("unknown gain map version"));
    }
    let n = r.varint()?;
    let metadata = r.vector(n, cap)?;
    zencodec::gainmap::parse_iso21496_fmt(&metadata, zencodec::Iso21496Format::JxlJhgm)
        .map_err(|_| Error::Corrupt("gain map parameters"))?;
    let flags = r.varint()?;
    if flags & !3 != 0 {
        return Err(Error::Unsupported("unknown gain map fields"));
    }
    if flags & 1 != 0 && r.bytes(4)?[3] > 1 {
        return Err(Error::Corrupt("gain map range flag"));
    }
    if flags & 2 != 0 {
        let n = r.varint()?;
        r.vector(n, cap)?;
    }
    Ok(r.pos..chunk.body.end)
}
