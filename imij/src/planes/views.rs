//! Native view discovery; backings reuse the primary or an existing plane level.
use super::*;
use crate::{ByteReader, Header, ImageInfo, put_varint};

/// Purpose of one logical image view. The original need not be the primary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageViewRole {
    Original,
    Reduced,
    Preview,
}

/// One backing authority. A level means its complete native packed backing,
/// including all compatible components, rather than only the selected plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ViewBacking {
    Primary,
    Level { plane: PlaneId, level: u32 },
}

/// Exact axis mapping: original edge coordinate = offset + scale * view edge
/// coordinate. Pixel centers have edge coordinate i + 1/2. Fractions are reduced;
/// scales are positive. This declaration performs no implicit resampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisRegistration {
    scale: (u64, u64),
    offset: (i64, u64),
}
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
impl AxisRegistration {
    pub fn new(
        scale_num: u64,
        scale_den: u64,
        offset_num: i64,
        offset_den: u64,
    ) -> Result<Self, Error> {
        if scale_num == 0 || scale_den == 0 || offset_den == 0 {
            return Err(Error::InvalidInput(
                "zero view registration scale or denominator",
            ));
        }
        let g = gcd(scale_num, scale_den);
        let o = gcd(offset_num.unsigned_abs(), offset_den);
        // Divide a signed numerator without narrowing a denominator of 2^63.
        let offset =
            i64::try_from(i128::from(offset_num) / i128::from(o)).map_err(|_| Error::TooLarge)?;
        Ok(Self {
            scale: (scale_num / g, scale_den / g),
            offset: (offset, offset_den / o),
        })
    }
    pub fn scale(self) -> (u64, u64) {
        self.scale
    }
    pub fn offset(self) -> (i64, u64) {
        self.offset
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewRegistration {
    pub x: AxisRegistration,
    pub y: AxisRegistration,
}
impl ViewRegistration {
    /// Explicit edge-aligned direct-original resize, including pixel-center
    /// registration. Call only when this matches the producer's resampling.
    pub fn resized(
        original_width: u32,
        original_height: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, Error> {
        Ok(Self {
            x: AxisRegistration::new(original_width.into(), width.into(), 0, 1)?,
            y: AxisRegistration::new(original_height.into(), height.into(), 0, 1)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageView {
    id: u64,
    name: String,
    role: ImageViewRole,
    backing: ViewBacking,
    registration: Option<ViewRegistration>,
}
impl ImageView {
    pub fn new(
        id: u64,
        name: impl Into<String>,
        role: ImageViewRole,
        backing: ViewBacking,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            role,
            backing,
            registration: None,
        }
    }
    /// Mapping into the named original's grid. None explicitly means unregistered.
    pub fn with_registration(mut self, registration: ViewRegistration) -> Self {
        self.registration = Some(registration);
        self
    }
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn role(&self) -> ImageViewRole {
        self.role
    }
    pub fn backing(&self) -> ViewBacking {
        self.backing
    }
    pub fn registration(&self) -> Option<ViewRegistration> {
        self.registration
    }
}

/// Bounded native image catalog. IDs are keys, including zero, and never size
/// arrays. Every mapping is to this directory's one original; no parent graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageDirectory {
    original: u64,
    views: Vec<ImageView>,
}
impl ImageDirectory {
    pub fn new(original: u64, views: Vec<ImageView>) -> Result<Self, Error> {
        Self::new_with_limits(original, views, PlaneSetLimits::default())
    }
    pub fn new_with_limits(
        original: u64,
        mut views: Vec<ImageView>,
        limits: PlaneSetLimits,
    ) -> Result<Self, Error> {
        if views.is_empty() || views.len() > limits.max_views {
            return Err(Error::TooLarge);
        }
        views.sort_unstable_by_key(|v| v.id);
        let result = Self { original, views };
        result.validate_ids(limits)?;
        Ok(result)
    }
    pub fn original(&self) -> u64 {
        self.original
    }
    pub fn views(&self) -> &[ImageView] {
        &self.views
    }
    pub fn view(&self, id: u64) -> Option<&ImageView> {
        self.views
            .binary_search_by_key(&id, |v| v.id)
            .ok()
            .map(|i| &self.views[i])
    }
    fn validate_ids(&self, limits: PlaneSetLimits) -> Result<(), Error> {
        if self.views.is_empty() || self.views.len() > limits.max_views {
            return Err(Error::TooLarge);
        }
        if self.views.windows(2).any(|v| v[0].id >= v[1].id) {
            return Err(Error::Corrupt("duplicate or unordered image view ID"));
        }
        if self
            .views
            .iter()
            .filter(|v| v.role == ImageViewRole::Original)
            .count()
            != 1
            || self
                .view(self.original)
                .is_none_or(|v| v.role != ImageViewRole::Original)
        {
            return Err(Error::Corrupt("native image original view"));
        }
        for view in &self.views {
            label(&view.name, limits)?;
            if view.role == ImageViewRole::Original && view.registration.is_some() {
                return Err(Error::Corrupt("original view has registration"));
            }
        }
        Ok(())
    }
    pub(super) fn validate(&self, dir: &Directory, limits: PlaneSetLimits) -> Result<(), Error> {
        self.validate_ids(limits)?;
        for view in &self.views {
            if let ViewBacking::Level { plane, level } = view.backing {
                backing_info(dir, plane, level)?;
            }
        }
        Ok(())
    }
    pub(super) fn encode(&self, limits: PlaneSetLimits) -> Result<Vec<u8>, Error> {
        self.validate_ids(limits)?;
        let mut out = Vec::new();
        out.try_reserve_exact(30).map_err(|_| Error::TooLarge)?;
        put_varint(&mut out, 1);
        put_varint(&mut out, self.original);
        put_varint(&mut out, self.views.len() as u64);
        for v in &self.views {
            put_varint(&mut out, v.id);
            put_varint(
                &mut out,
                match v.role {
                    ImageViewRole::Original => 0,
                    ImageViewRole::Reduced => 1,
                    ImageViewRole::Preview => 2,
                },
            );
            put_varint(&mut out, v.name.len() as u64);
            out.try_reserve(v.name.len() + 96)
                .map_err(|_| Error::TooLarge)?;
            out.extend_from_slice(v.name.as_bytes());
            match v.backing {
                ViewBacking::Primary => put_varint(&mut out, 0),
                ViewBacking::Level { plane, level } => {
                    put_varint(&mut out, 1);
                    put_varint(&mut out, plane.0.into());
                    put_varint(&mut out, level.into());
                }
            }
            put_varint(&mut out, u64::from(v.registration.is_some()));
            if let Some(mapping) = v.registration {
                for axis in [mapping.x, mapping.y] {
                    for n in [
                        axis.scale.0,
                        axis.scale.1,
                        ((axis.offset.0 as u64) << 1) ^ ((axis.offset.0 >> 63) as u64),
                        axis.offset.1,
                    ] {
                        put_varint(&mut out, n)
                    }
                }
            }
            if out.len() > limits.max_index_bytes {
                return Err(Error::TooLarge);
            }
        }
        Ok(out)
    }
    pub(super) fn read(bytes: &[u8], limits: PlaneSetLimits) -> Result<Self, Error> {
        let mut r = ByteReader::new(bytes);
        if r.varint()? != 1 {
            return Err(Error::Unsupported("native image directory version"));
        }
        let original = r.varint()?;
        let count = r.var_usize()?;
        if count == 0 || count > limits.max_views {
            return Err(Error::TooLarge);
        }
        if count > r.remaining() / 5 {
            return Err(Error::Corrupt("image view count exceeds directory"));
        }
        let mut views = Vec::new();
        views
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for _ in 0..count {
            let id = r.varint()?;
            let role = match r.varint()? {
                0 => ImageViewRole::Original,
                1 => ImageViewRole::Reduced,
                2 => ImageViewRole::Preview,
                _ => return Err(Error::Unsupported("native image view role")),
            };
            let length = r.var_usize()?;
            if length > limits.max_label_bytes {
                return Err(Error::TooLarge);
            }
            let text = core::str::from_utf8(r.bytes(length)?)
                .map_err(|_| Error::Corrupt("view label UTF8"))?;
            let mut name = String::new();
            name.try_reserve_exact(length)
                .map_err(|_| Error::TooLarge)?;
            name.push_str(text);
            let backing = match r.varint()? {
                0 => ViewBacking::Primary,
                1 => ViewBacking::Level {
                    plane: PlaneId(r.var_u32()?),
                    level: r.var_u32()?,
                },
                _ => return Err(Error::Unsupported("native view backing")),
            };
            let registration = match r.varint()? {
                0 => None,
                1 => {
                    let mut axis = || -> Result<AxisRegistration, Error> {
                        let n = r.varint()?;
                        let d = r.varint()?;
                        let z = r.varint()?;
                        let off = ((z >> 1) as i64) ^ -((z & 1) as i64);
                        let od = r.varint()?;
                        let value = AxisRegistration::new(n, d, off, od)
                            .map_err(|_| Error::Corrupt("view registration"))?;
                        if value.scale != (n, d) || value.offset != (off, od) {
                            return Err(Error::Corrupt("noncanonical view registration"));
                        }
                        Ok(value)
                    };
                    Some(ViewRegistration {
                        x: axis()?,
                        y: axis()?,
                    })
                }
                _ => return Err(Error::Corrupt("view registration flag")),
            };
            views.push(ImageView {
                id,
                name,
                role,
                backing,
                registration,
            });
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("image directory trailing bytes"));
        }
        let value = Self { original, views };
        value.validate_ids(limits)?;
        Ok(value)
    }
}

fn level_ref(
    dir: &Directory,
    plane: PlaneId,
    level: u32,
) -> Result<(&PlaneDescriptor, &LevelDescriptor), Error> {
    let p = dir
        .planes
        .get(plane.0 as usize)
        .ok_or(Error::Corrupt("native view plane ID"))?;
    let l = p
        .levels
        .get(level as usize)
        .ok_or(Error::Corrupt("native view level ID"))?;
    Ok((p, l))
}
pub(super) fn backing_info(
    dir: &Directory,
    plane: PlaneId,
    level: u32,
) -> Result<(ImageInfo, [(PlaneId, u32); 4]), Error> {
    let (p, l) = level_ref(dir, plane, level)?;
    if l.channels.count() > 4 {
        return Err(Error::Unsupported("native view component count"));
    }
    let mut components = [(PlaneId(0), 0); 4];
    let mut bits = [0; 4];
    let mut mask = 0;
    for (pi, other) in dir.planes.iter().enumerate() {
        for (li, ol) in other.levels.iter().enumerate() {
            if ol.record_start != l.record_start {
                continue;
            }
            if other.sample != p.sample
                || other.cicp != p.cicp
                || other.group != p.group
                || other.origin != p.origin
                || other.icc_profile != p.icc_profile
                || (
                    ol.width,
                    ol.height,
                    ol.tile_width,
                    ol.tile_height,
                    ol.channels,
                ) != (l.width, l.height, l.tile_width, l.tile_height, l.channels)
                || ol.component as usize >= l.channels.count()
                || mask & (1 << ol.component) != 0
            {
                return Err(Error::Corrupt("native view component backing"));
            }
            components[ol.component as usize] = (PlaneId(pi as u32), li as u32);
            bits[ol.component as usize] = other.precision;
            mask |= 1 << ol.component;
        }
    }
    if mask != (1 << l.channels.count()) - 1 {
        return Err(Error::Corrupt("native view missing components"));
    }
    let mut info = ImageInfo::new(l.width, l.height, l.channels, p.sample)
        .with_significant_bits(&bits[..l.channels.count()])?;
    info.cicp = p.cicp;
    Ok((info, components))
}

#[cfg(feature = "encode")]
impl PlaneSetBuilder {
    pub fn set_image_directory(&mut self, directory: ImageDirectory) -> Result<(), Error> {
        if self.version != 4 {
            return Err(Error::InvalidInput("native image directory requires V4"));
        }
        directory.validate(&self.dir, self.limits)?;
        self.dir.images = Some(directory);
        Ok(())
    }
}
impl<R: ReadAt> PlaneSetReader<R> {
    pub fn image_directory(&self) -> Option<&ImageDirectory> {
        self.dir.images.as_ref()
    }
    /// Original stored interpretation; orientation/ICC metadata is not applied.
    pub fn view_info(&mut self, id: u64) -> Result<ImageInfo, Error> {
        let backing = self.view_backing(id)?;
        match backing {
            ViewBacking::Primary => Ok(self.primary_header()?.info),
            ViewBacking::Level { plane, level } => Ok(backing_info(&self.dir, plane, level)?.0),
        }
    }
    /// Return the stored primary header without loading primary pixel entropy.
    pub fn primary_header(&mut self) -> Result<Header, Error> {
        let n = self.dir.primary_len.min(64) as usize;
        let mut prefix = [0; 64];
        self.read_at(0, &mut prefix[..n])?;
        let mut r = ByteReader::new(&prefix[..n]);
        r.bytes(8)?;
        let length = r.var_usize()?;
        let total = r
            .pos
            .checked_add(length)
            .filter(|&v| v as u64 <= self.dir.primary_len)
            .ok_or(Error::Corrupt("native primary header bounds"))?;
        if length > 64 << 10 {
            return Err(Error::TooLarge);
        }
        if total <= n {
            return Ok(crate::parse::header::read_versioned(
                &mut ByteReader::new(&prefix[..total]),
                64 << 10,
            )?
            .header);
        }
        let mut bytes = super::decode::buffer(total)?;
        self.read_at(0, &mut bytes)?;
        Ok(crate::parse::header::read_versioned(&mut ByteReader::new(&bytes), 64 << 10)?.header)
    }
    pub fn decode_view_region(
        &mut self,
        id: u64,
        rect: Rect,
        output: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        let backing = self.view_backing(id)?;
        match backing {
            ViewBacking::Level { plane, level } => {
                let (info, components) = backing_info(&self.dir, plane, level)?;
                self.decode_color_levels(&components[..info.channels.count()], rect, output, stride)
            }
            ViewBacking::Primary => self.with_read_request(|reader| {
                let workers = reader.limits.decode_workers;
                reader
                    .primary_region_reader()?
                    .decode_region_threads(rect, output, stride, workers)
            }),
        }
    }
    pub(super) fn view_backing(&self, id: u64) -> Result<ViewBacking, Error> {
        self.dir
            .images
            .as_ref()
            .and_then(|directory| directory.view(id))
            .map(ImageView::backing)
            .ok_or(Error::InvalidInput("image view ID"))
    }

    pub(super) fn primary_region_reader(
        &mut self,
    ) -> Result<crate::region::RegionReader<PrimarySource<'_, R>>, Error> {
        // The outer reader verifies the checksum over the complete file.
        let limits = crate::Limits {
            max_pixels: self.limits.max_plane_pixels,
            max_bytes: self.limits.max_compressed_work,
            verify_checksum: false,
        };
        let verify = self.limits.verify_scoped_pixels;
        let profile = self.limits.v4_profile;
        let mut reader = crate::region::RegionReader::open_profiled(
            PrimarySource { reader: self },
            &limits,
            profile,
        )?;
        reader.set_scoped_pixel_verification(verify);
        Ok(reader)
    }
}
pub(super) struct PrimarySource<'a, R: ReadAt> {
    reader: &'a mut PlaneSetReader<R>,
}
impl<R: ReadAt> ReadAt for PrimarySource<'_, R> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.reader.dir.primary_len)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|v| v > self.reader.dir.primary_len)
        {
            return Err(Error::Corrupt("view primary read bounds"));
        }
        self.reader.read_at(offset, out)
    }
}
