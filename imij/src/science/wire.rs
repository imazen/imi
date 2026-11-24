//! Wire form and validation of the science records (SPEC.md 1.5).

use alloc::{string::String, vec::Vec};

use super::*;
use crate::{ByteReader, Error, put_varint};

pub(super) const VERSION: u8 = 1;
const MAX_TEXT: usize = 256;
const MAX_SCHEMA: usize = 64;
const MAX_AXES: usize = 32;
const MAX_LEVELS: usize = 64;
const MAX_NODATA: usize = 16;
const MAX_STRUCTURED: usize = 64;
const MAX_CATEGORIES: usize = 65_536;
const MAX_DESCRIPTION: usize = 1024;

// Calibration entry flags.
const C_LINEAR: u64 = 1;
const C_UNIT: u64 = 1 << 1;
const C_QUANTITY: u64 = 1 << 2;
const C_STANDARD_NAME: u64 = 1 << 3;
const C_VALID_MIN: u64 = 1 << 4;
const C_VALID_MAX: u64 = 1 << 5;
const C_NAN_MISSING: u64 = 1 << 6;
const C_MASK: u64 = 1 << 7;
const C_INTEGER_NODATA: u64 = 1 << 8;
// Axes record flags.
const A_AREA: u64 = 1;
const A_AFFINE: u64 = 1 << 1;
const A_METHOD: u64 = 1 << 2;
// Categories record flags, then entry flags.
const G_COLOR16: u64 = 1;
const E_COLOR: u64 = 1;
const E_NAME: u64 = 1 << 1;
const E_DESCRIPTION: u64 = 1 << 2;

/// Raw record payloads collected for one target, before decoding.
#[derive(Default)]
pub(super) struct Parts<'a> {
    calibration: Option<&'a [u8]>,
    axes: Option<&'a [u8]>,
    coordinates: Option<&'a [u8]>,
    structured: Vec<&'a [u8]>,
    categories: Option<&'a [u8]>,
}

impl<'a> Parts<'a> {
    /// Adds a record; other tags are ignored. A repeated record is corrupt.
    pub(super) fn add(&mut self, tag: [u8; 4], payload: &'a [u8]) -> Result<(), Error> {
        let slot = match tag {
            CALB => &mut self.calibration,
            AXES => &mut self.axes,
            CORD => &mut self.coordinates,
            CATG => &mut self.categories,
            SMDT => {
                if self.structured.len() >= MAX_STRUCTURED {
                    return Err(Error::Corrupt("too many structured metadata records"));
                }
                self.structured.push(payload);
                return Ok(());
            }
            _ => return Ok(()),
        };
        if slot.is_some() {
            return Err(Error::Corrupt("repeated science record"));
        }
        *slot = Some(payload);
        Ok(())
    }

    fn has(&self, tag: [u8; 4]) -> bool {
        match tag {
            CALB => self.calibration.is_some(),
            AXES => self.axes.is_some(),
            CORD => self.coordinates.is_some(),
            CATG => self.categories.is_some(),
            _ => !self.structured.is_empty(),
        }
    }

    /// Takes the records of `tag` from `other` (a plane's own records
    /// replace the plane set's defaults tag by tag).
    fn inherit(&mut self, other: &Parts<'a>, tag: [u8; 4]) {
        match tag {
            CALB => self.calibration = other.calibration,
            AXES => self.axes = other.axes,
            CORD => self.coordinates = other.coordinates,
            CATG => self.categories = other.categories,
            _ => self.structured.clone_from(&other.structured),
        }
    }

    /// Decodes and cross-checks the records.
    pub(super) fn decode(&self, target: &Target) -> Result<ScienceRecords, Error> {
        let mut out = ScienceRecords::default();
        if let Some(p) = self.calibration {
            out.calibration = decode_calibration(p)?.unwrap_or_default();
        }
        let mut axes_skipped = false;
        if let Some(p) = self.axes {
            out.axes = decode_axes(p)?;
            axes_skipped = out.axes.is_none();
        }
        if let Some(p) = self.coordinates {
            let mut r = ByteReader::new(p);
            if r.u8()? == VERSION {
                let n = r.var_usize()?;
                if n == 0 || n > MAX_AXES {
                    return Err(Error::Corrupt("coordinate count"));
                }
                for _ in 0..n {
                    let z = r.varint()?;
                    out.coordinates.push(((z >> 1) as i64) ^ -((z & 1) as i64));
                }
                end(&r)?;
            }
        }
        for p in &self.structured {
            if let Some(s) = decode_structured(p)? {
                out.structured.push(s);
            }
        }
        if let Some(p) = self.categories {
            out.categories = decode_categories(p)?.unwrap_or_default();
        }
        // Coordinates without known axes cannot be interpreted: skip them
        // when the axes record was skipped (a newer version).
        if axes_skipped {
            out.coordinates.clear();
        }
        validate_with(&out, target, out.axes.as_ref())?;
        Ok(out)
    }
}

pub(super) fn format_byte(f: StructuredFormat) -> u8 {
    match f {
        StructuredFormat::Text => 0,
        StructuredFormat::Json => 1,
        StructuredFormat::Cbor => 2,
    }
}

pub(super) fn put_str(out: &mut Vec<u8>, s: &str) {
    put_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn bit(on: bool, flag: u64) -> u64 {
    if on { flag } else { 0 }
}

pub(super) fn encode_calibration(cal: &[ChannelCalibration]) -> Vec<u8> {
    let version = if cal.iter().any(|c| c.integer_nodata.is_some()) {
        2
    } else {
        VERSION
    };
    let mut b = alloc::vec![version, cal.len() as u8];
    for c in cal {
        let linear = c.slope != 1.0 || c.offset != 0.0 || c.offset.is_sign_negative();
        let flags = bit(linear, C_LINEAR)
            | bit(c.unit.is_some(), C_UNIT)
            | bit(c.quantity.is_some(), C_QUANTITY)
            | bit(c.quantity_is_standard_name, C_STANDARD_NAME)
            | bit(c.valid_min.is_some(), C_VALID_MIN)
            | bit(c.valid_max.is_some(), C_VALID_MAX)
            | bit(c.nan_is_missing, C_NAN_MISSING)
            | bit(c.mask_plane.is_some(), C_MASK)
            | bit(c.integer_nodata.is_some(), C_INTEGER_NODATA);
        put_varint(&mut b, flags);
        if linear {
            put_f64(&mut b, c.slope);
            put_f64(&mut b, c.offset);
        }
        for s in [&c.unit, &c.quantity].into_iter().flatten() {
            put_str(&mut b, s);
        }
        for v in [c.valid_min, c.valid_max].into_iter().flatten() {
            put_f64(&mut b, v);
        }
        if let Some(m) = c.mask_plane {
            put_varint(&mut b, m.into());
        }
        match &c.integer_nodata {
            Some(IntegerNodata::Unsigned(values)) => {
                b.push(0);
                put_varint(&mut b, values.len() as u64);
                for value in values {
                    b.extend_from_slice(&value.to_le_bytes());
                }
            }
            Some(IntegerNodata::Signed(values)) => {
                b.push(1);
                put_varint(&mut b, values.len() as u64);
                for value in values {
                    b.extend_from_slice(&value.to_le_bytes());
                }
            }
            None => {
                put_varint(&mut b, c.nodata.len() as u64);
                for &v in &c.nodata {
                    put_f64(&mut b, v);
                }
            }
        }
    }
    b
}

pub(super) fn encode_axes(a: &Axes) -> Vec<u8> {
    let mut b = alloc::vec![VERSION];
    let flags = bit(a.pixel_is_area, A_AREA)
        | bit(a.affine.is_some(), A_AFFINE)
        | bit(a.downsampling.is_some(), A_METHOD);
    put_varint(&mut b, flags);
    b.push(a.axes.len() as u8);
    for x in &a.axes {
        put_str(&mut b, &x.name);
        match &x.kind {
            AxisKind::Space => b.push(0),
            AxisKind::Time => b.push(1),
            AxisKind::Channel => b.push(2),
            AxisKind::Other(t) => {
                b.push(3);
                put_str(&mut b, t);
            }
        }
        b.push(match x.binding {
            AxisBinding::Coordinate => 0,
            AxisBinding::Columns => 1,
            AxisBinding::Rows => 2,
            AxisBinding::Channels => 3,
            AxisBinding::Frames => 4,
        });
        put_str(&mut b, x.unit.as_deref().unwrap_or(""));
    }
    put_varint(&mut b, a.levels.len() as u64);
    for l in &a.levels {
        for &v in l.scale.iter().chain(&l.translation) {
            put_f64(&mut b, v);
        }
    }
    if let Some(m) = &a.downsampling {
        put_str(&mut b, m);
    }
    for &v in a.affine.iter().flatten() {
        put_f64(&mut b, v);
    }
    b
}

fn end(r: &ByteReader<'_>) -> Result<(), Error> {
    if r.remaining() != 0 {
        return Err(Error::Corrupt("science record trailing bytes"));
    }
    Ok(())
}

fn read_f64(r: &mut ByteReader<'_>) -> Result<f64, Error> {
    Ok(f64::from_le_bytes(r.bytes(8)?.try_into().unwrap()))
}

fn read_str(r: &mut ByteReader<'_>, max: usize) -> Result<String, Error> {
    let n = r.var_usize()?;
    if n > max {
        return Err(Error::Corrupt("science record string length"));
    }
    let s = core::str::from_utf8(r.bytes(n)?)
        .map_err(|_| Error::Corrupt("science record string UTF-8"))?;
    Ok(String::from(s))
}

fn decode_calibration(p: &[u8]) -> Result<Option<Vec<ChannelCalibration>>, Error> {
    let mut r = ByteReader::new(p);
    let version = r.u8()?;
    if !matches!(version, VERSION | 2) {
        return Ok(None);
    }
    let n = r.u8()? as usize;
    if !(1..=4).contains(&n) {
        return Err(Error::Corrupt("calibration channel count"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let flags = r.varint()?;
        if flags >> (if version == 2 { 9 } else { 8 }) != 0 {
            return Err(Error::Corrupt("calibration reserved flags"));
        }
        let mut c = ChannelCalibration::default();
        if flags & C_LINEAR != 0 {
            c.slope = read_f64(&mut r)?;
            c.offset = read_f64(&mut r)?;
        }
        if flags & C_UNIT != 0 {
            c.unit = Some(read_str(&mut r, MAX_TEXT)?);
        }
        if flags & C_QUANTITY != 0 {
            c.quantity = Some(read_str(&mut r, MAX_TEXT)?);
        }
        c.quantity_is_standard_name = flags & C_STANDARD_NAME != 0;
        if flags & C_VALID_MIN != 0 {
            c.valid_min = Some(read_f64(&mut r)?);
        }
        if flags & C_VALID_MAX != 0 {
            c.valid_max = Some(read_f64(&mut r)?);
        }
        c.nan_is_missing = flags & C_NAN_MISSING != 0;
        if flags & C_MASK != 0 {
            c.mask_plane = Some(r.var_u32()?);
        }
        let integer_kind = if flags & C_INTEGER_NODATA != 0 {
            Some(r.u8()?)
        } else {
            None
        };
        let k = r.var_usize()?;
        if k > MAX_NODATA {
            return Err(Error::Corrupt("too many nodata values"));
        }
        match integer_kind {
            Some(0) => {
                let mut values = Vec::with_capacity(k);
                for _ in 0..k {
                    values.push(u64::from_le_bytes(r.bytes(8)?.try_into().unwrap()));
                }
                c.integer_nodata = Some(IntegerNodata::Unsigned(values));
            }
            Some(1) => {
                let mut values = Vec::with_capacity(k);
                for _ in 0..k {
                    values.push(i64::from_le_bytes(r.bytes(8)?.try_into().unwrap()));
                }
                c.integer_nodata = Some(IntegerNodata::Signed(values));
            }
            Some(_) => return Err(Error::Corrupt("integer nodata kind")),
            None => {
                for _ in 0..k {
                    c.nodata.push(read_f64(&mut r)?);
                }
            }
        }
        out.push(c);
    }
    end(&r)?;
    Ok(Some(out))
}

fn decode_axes(p: &[u8]) -> Result<Option<Axes>, Error> {
    let mut r = ByteReader::new(p);
    if r.u8()? != VERSION {
        return Ok(None);
    }
    let flags = r.varint()?;
    if flags >> 3 != 0 {
        return Err(Error::Corrupt("axes reserved flags"));
    }
    let n = r.u8()? as usize;
    if !(2..=MAX_AXES).contains(&n) {
        return Err(Error::Corrupt("axis count"));
    }
    let mut axes = Vec::with_capacity(n);
    for _ in 0..n {
        let name = read_str(&mut r, MAX_TEXT)?;
        let kind = match r.u8()? {
            0 => AxisKind::Space,
            1 => AxisKind::Time,
            2 => AxisKind::Channel,
            3 => AxisKind::Other(read_str(&mut r, MAX_TEXT)?),
            _ => return Err(Error::Corrupt("axis kind")),
        };
        let binding = match r.u8()? {
            0 => AxisBinding::Coordinate,
            1 => AxisBinding::Columns,
            2 => AxisBinding::Rows,
            3 => AxisBinding::Channels,
            4 => AxisBinding::Frames,
            _ => return Err(Error::Corrupt("axis binding")),
        };
        let unit = read_str(&mut r, MAX_TEXT)?;
        axes.push(Axis {
            name,
            kind,
            binding,
            unit: (!unit.is_empty()).then_some(unit),
        });
    }
    let levels = r.var_usize()?;
    if levels > MAX_LEVELS {
        return Err(Error::Corrupt("axis level count"));
    }
    let mut a = Axes::new(axes);
    a.pixel_is_area = flags & A_AREA != 0;
    for _ in 0..levels {
        let mut v = Vec::with_capacity(2 * n);
        for _ in 0..2 * n {
            v.push(read_f64(&mut r)?);
        }
        let translation = v.split_off(n);
        a.levels.push(AxisLevel::new(v, translation));
    }
    if flags & A_METHOD != 0 {
        a.downsampling = Some(read_str(&mut r, MAX_TEXT)?);
    }
    if flags & A_AFFINE != 0 {
        let mut m = Vec::with_capacity(n * (n + 1));
        for _ in 0..n * (n + 1) {
            m.push(read_f64(&mut r)?);
        }
        a.affine = Some(m);
    }
    end(&r)?;
    Ok(Some(a))
}

/// `cATG`: per channel a count and entries in strictly increasing value
/// order; colors are 8-bit unless the record flag says 16-bit.
pub(super) fn encode_categories(channels: &[Vec<Category>]) -> Vec<u8> {
    let wide = channels
        .iter()
        .flatten()
        .filter_map(|c| c.color)
        .flatten()
        .any(|v| v % 257 != 0);
    let mut b = alloc::vec![VERSION];
    put_varint(&mut b, bit(wide, G_COLOR16));
    b.push(channels.len() as u8);
    for list in channels {
        put_varint(&mut b, list.len() as u64);
        for c in list {
            put_varint(&mut b, ((c.value << 1) ^ (c.value >> 63)) as u64);
            let flags = bit(c.color.is_some(), E_COLOR)
                | bit(c.name.is_some(), E_NAME)
                | bit(c.description.is_some(), E_DESCRIPTION);
            put_varint(&mut b, flags);
            for v in c.color.into_iter().flatten() {
                if wide {
                    b.extend_from_slice(&v.to_le_bytes());
                } else {
                    b.push((v / 257) as u8);
                }
            }
            for s in [&c.name, &c.description].into_iter().flatten() {
                put_str(&mut b, s);
            }
        }
    }
    b
}

fn decode_categories(p: &[u8]) -> Result<Option<Vec<Vec<Category>>>, Error> {
    let mut r = ByteReader::new(p);
    if r.u8()? != VERSION {
        return Ok(None);
    }
    let flags = r.varint()?;
    if flags >> 1 != 0 {
        return Err(Error::Corrupt("categories reserved flags"));
    }
    let wide = flags & G_COLOR16 != 0;
    let n = r.u8()? as usize;
    if !(1..=4).contains(&n) {
        return Err(Error::Corrupt("categories channel count"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let k = r.var_usize()?;
        // Each entry takes at least two bytes: bound before allocating.
        if k > MAX_CATEGORIES || k > r.remaining() / 2 {
            return Err(Error::Corrupt("category count"));
        }
        let mut list = Vec::with_capacity(k);
        for _ in 0..k {
            let z = r.varint()?;
            let mut c = Category::new(((z >> 1) as i64) ^ -((z & 1) as i64));
            let e = r.varint()?;
            if e >> 3 != 0 {
                return Err(Error::Corrupt("category reserved flags"));
            }
            if e & E_COLOR != 0 {
                let mut rgba = [0u16; 4];
                for v in &mut rgba {
                    *v = if wide {
                        r.u16()?
                    } else {
                        u16::from(r.u8()?) * 257
                    };
                }
                c.color = Some(rgba);
            }
            if e & E_NAME != 0 {
                c.name = Some(read_str(&mut r, MAX_TEXT)?);
            }
            if e & E_DESCRIPTION != 0 {
                c.description = Some(read_str(&mut r, MAX_DESCRIPTION)?);
            }
            list.push(c);
        }
        out.push(list);
    }
    end(&r)?;
    Ok(Some(out))
}

/// Borrowed `sMDT` identity, available before the packed metadata body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StructuredIdentity<'a> {
    pub schema: &'a str,
    pub instance_key: Option<&'a str>,
    pub format: StructuredFormat,
}

/// Reads an `sMDT` payload prefix without inspecting its packed body.
/// Returns `None` for an unknown version or format. A known identity needs
/// at most 132 payload bytes, including both length fields.
pub fn structured_identity(payload_prefix: &[u8]) -> Result<Option<StructuredIdentity<'_>>, Error> {
    Ok(structured_head(payload_prefix)?.map(|(identity, _)| identity))
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_SCHEMA && value.bytes().all(|b| b.is_ascii_graphic())
}

fn read_identity<'a>(r: &mut ByteReader<'a>) -> Result<&'a str, Error> {
    let len = r.var_usize()?;
    if !(1..=MAX_SCHEMA).contains(&len) {
        return Err(Error::Corrupt("structured metadata identity length"));
    }
    let value = core::str::from_utf8(r.bytes(len)?)
        .map_err(|_| Error::Corrupt("structured metadata identity"))?;
    if !valid_identity(value) {
        return Err(Error::Corrupt("structured metadata identity"));
    }
    Ok(value)
}

fn structured_head(p: &[u8]) -> Result<Option<(StructuredIdentity<'_>, &[u8])>, Error> {
    let mut r = ByteReader::new(p);
    let version = r.u8()?;
    if !matches!(version, VERSION | 2) {
        return Ok(None);
    }
    let format = match r.u8()? {
        0 => StructuredFormat::Text,
        1 => StructuredFormat::Json,
        2 => StructuredFormat::Cbor,
        _ => return Ok(None),
    };
    let schema = read_identity(&mut r)?;
    let instance_key = if version == 2 {
        Some(read_identity(&mut r)?)
    } else {
        None
    };
    Ok(Some((
        StructuredIdentity {
            schema,
            instance_key,
            format,
        },
        &p[r.pos..],
    )))
}

fn decode_structured(p: &[u8]) -> Result<Option<StructuredMetadata>, Error> {
    let Some((identity, body)) = structured_head(p)? else {
        return Ok(None);
    };
    let data = crate::metadata_pack::unpack(body, crate::parse::chunks::MAX_PACKED_METADATA)?;
    Ok(Some(StructuredMetadata {
        schema: String::from(identity.schema),
        instance_key: identity.instance_key.map(String::from),
        format: identity.format,
        data,
    }))
}

fn text(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_TEXT && !s.contains('\0')
}

/// Checks every rule of SPEC.md 1.5. `axes` are the effective axes the
/// coordinates refer to (a plane may inherit them from the defaults).
pub(super) fn validate_with(
    rec: &ScienceRecords,
    target: &Target,
    axes: Option<&Axes>,
) -> Result<(), Error> {
    if !rec.calibration.is_empty() && rec.calibration.len() != target.channels {
        return Err(Error::Corrupt("calibration channel count"));
    }
    for c in &rec.calibration {
        if !c.slope.is_finite() || c.slope == 0.0 || !c.offset.is_finite() {
            return Err(Error::Corrupt("calibration slope or offset"));
        }
        if c.unit.as_deref().is_some_and(|s| !text(s))
            || c.quantity.as_deref().is_some_and(|s| !text(s))
            || (c.quantity_is_standard_name && c.quantity.is_none())
        {
            return Err(Error::Corrupt("calibration unit or quantity"));
        }
        if c.nodata.len() > MAX_NODATA || c.nodata.iter().any(|v| v.is_nan()) {
            return Err(Error::Corrupt("calibration nodata"));
        }
        if let Some(integer) = &c.integer_nodata {
            if !c.nodata.is_empty() || !(1..=MAX_NODATA).contains(&integer.len()) {
                return Err(Error::Corrupt("integer nodata count"));
            }
            if let Some(sample) = target.sample {
                let valid = match integer {
                    IntegerNodata::Unsigned(values) => {
                        !sample.is_float()
                            && !sample.is_signed()
                            && values
                                .iter()
                                .all(|&value| value <= u64::MAX >> (64 - sample.depth()))
                    }
                    IntegerNodata::Signed(values) => {
                        let max = i64::MAX >> (64 - sample.depth());
                        sample.is_signed()
                            && values.iter().all(|&value| (!max..=max).contains(&value))
                    }
                };
                if !valid {
                    return Err(Error::Corrupt("integer nodata sample type or range"));
                }
            }
        }
        if c.valid_min.is_some_and(f64::is_nan)
            || c.valid_max.is_some_and(f64::is_nan)
            || matches!((c.valid_min, c.valid_max), (Some(a), Some(b)) if a > b)
        {
            return Err(Error::Corrupt("calibration valid range"));
        }
        if let (Some(m), Some(n)) = (c.mask_plane, target.planes)
            && m >= n
        {
            return Err(Error::Corrupt("calibration mask plane"));
        }
    }
    if let Some(a) = &rec.axes {
        let n = a.axes.len();
        if !(2..=MAX_AXES).contains(&n) || a.levels.len() > MAX_LEVELS {
            return Err(Error::Corrupt("axis or level count"));
        }
        let count = |b: AxisBinding| a.axes.iter().filter(|x| x.binding == b).count();
        if count(AxisBinding::Columns) != 1
            || count(AxisBinding::Rows) != 1
            || count(AxisBinding::Channels) > usize::from(target.image)
            || count(AxisBinding::Frames) > usize::from(target.image)
        {
            return Err(Error::Corrupt("axis bindings"));
        }
        for (i, x) in a.axes.iter().enumerate() {
            if !text(&x.name)
                || a.axes[..i].iter().any(|y| y.name == x.name)
                || x.unit.as_deref().is_some_and(|s| !text(s))
                || matches!(&x.kind, AxisKind::Other(t) if t.len() > MAX_TEXT || t.contains('\0'))
            {
                return Err(Error::Corrupt("axis name, type or unit"));
            }
        }
        for l in &a.levels {
            if l.scale.len() != n
                || l.translation.len() != n
                || l.scale.iter().any(|v| !v.is_finite() || *v == 0.0)
                || l.translation.iter().any(|v| !v.is_finite())
            {
                return Err(Error::Corrupt("axis level transform"));
            }
        }
        if a.affine
            .as_ref()
            .is_some_and(|m| m.len() != n * (n + 1) || m.iter().any(|v| !v.is_finite()))
        {
            return Err(Error::Corrupt("axis affine"));
        }
        if a.downsampling.as_deref().is_some_and(|s| !text(s)) {
            return Err(Error::Corrupt("axis downsampling method"));
        }
    }
    if !rec.coordinates.is_empty()
        && (target.default
            || rec.coordinates.len() > MAX_AXES
            || axes.is_none_or(|a| a.coordinate_axes() != rec.coordinates.len()))
    {
        return Err(Error::Corrupt("coordinates do not match the axes"));
    }
    if !rec.categories.is_empty() {
        if rec.categories.len() != target.channels || rec.categories.iter().all(|l| l.is_empty()) {
            return Err(Error::Corrupt("categories channel count"));
        }
        for (ch, list) in rec.categories.iter().enumerate() {
            if list.len() > MAX_CATEGORIES {
                return Err(Error::Corrupt("category count"));
            }
            if !list.is_empty()
                && rec
                    .calibration
                    .get(ch)
                    .is_some_and(|c| c.slope != 1.0 || c.offset != 0.0)
            {
                return Err(Error::Corrupt("categorical channel with slope or offset"));
            }
            for (i, c) in list.iter().enumerate() {
                if (i > 0 && list[i - 1].value >= c.value)
                    || c.name.as_deref().is_some_and(|s| !text(s))
                    || c.description.as_deref().is_some_and(|s| {
                        s.is_empty() || s.len() > MAX_DESCRIPTION || s.contains('\0')
                    })
                {
                    return Err(Error::Corrupt("category order, name or description"));
                }
            }
        }
    }
    if rec.structured.len() > MAX_STRUCTURED {
        return Err(Error::Corrupt("too many structured metadata records"));
    }
    for (i, s) in rec.structured.iter().enumerate() {
        if !valid_identity(&s.schema)
            || rec.structured[..i]
                .iter()
                .any(|t| t.schema == s.schema && t.instance_key == s.instance_key)
        {
            return Err(Error::Corrupt("structured metadata schema"));
        }
        if s.instance_key
            .as_deref()
            .is_some_and(|key| !valid_identity(key))
        {
            return Err(Error::Corrupt("structured metadata instance key"));
        }
        if s.data.len() > crate::parse::chunks::MAX_PACKED_METADATA {
            return Err(Error::TooLarge);
        }
        if s.format != StructuredFormat::Cbor && core::str::from_utf8(&s.data).is_err() {
            return Err(Error::Corrupt("structured metadata text is not UTF-8"));
        }
    }
    Ok(())
}

/// Serializes plane-set records: `(target, tag, varint(len + 1), payload)`
/// entries, target 0 = defaults for every plane, `k + 1` = plane `k`.
/// `entries` are sorted by target with at most one entry per target.
pub(crate) fn encode_plane_entries(
    entries: &[(u32, ScienceRecords)],
    planes: u32,
) -> Result<Vec<u8>, Error> {
    let default_axes = entries
        .iter()
        .find(|(t, _)| *t == 0)
        .and_then(|(_, r)| r.axes.as_ref());
    let mut out = Vec::new();
    for (target, rec) in entries {
        let ctx = Target::plane(planes, *target == 0, None);
        for (tag, payload) in rec.encode_for(&ctx, rec.axes.as_ref().or(default_axes))? {
            put_varint(&mut out, (*target).into());
            out.extend_from_slice(&tag);
            put_varint(&mut out, payload.len() as u64 + 1);
            out.extend_from_slice(&payload);
        }
    }
    Ok(out)
}

/// The effective records of `plane` in a plane-set science attachment: per
/// tag, the plane's own records if it has any, else the defaults.
pub(crate) fn decode_plane(
    bytes: &[u8],
    planes: u32,
    plane: u32,
    sample: Option<crate::SampleType>,
) -> Result<ScienceRecords, Error> {
    let mut default = Parts::default();
    let mut own = Parts::default();
    plane_records(bytes, planes, |target, tag, payload| {
        if target == 0 {
            default.add(tag, payload)?;
        } else if target - 1 == u64::from(plane) {
            own.add(tag, payload)?;
        }
        Ok(())
    })?;
    // Malformed defaults are corrupt even where a plane overrides them.
    default.decode(&Target::plane(planes, true, None))?;
    let mut parts = Parts::default();
    for tag in [CALB, AXES, CORD, SMDT, CATG] {
        parts.inherit(if own.has(tag) { &own } else { &default }, tag);
    }
    parts.decode(&Target::plane(planes, false, sample))
}

fn plane_records<'a>(
    bytes: &'a [u8],
    planes: u32,
    mut visit: impl FnMut(u64, [u8; 4], &'a [u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut r = ByteReader::new(bytes);
    while r.remaining() != 0 {
        let target = r.varint()?;
        if target > u64::from(planes) {
            return Err(Error::Corrupt("science record target"));
        }
        let tag: [u8; 4] = r.bytes(4)?.try_into().unwrap();
        if !tag.iter().all(u8::is_ascii_alphabetic) {
            return Err(Error::Corrupt("science record tag"));
        }
        let n = r.varint()?;
        if n == 0 {
            return Err(Error::Corrupt("science record length"));
        }
        let payload = r.bytes(usize::try_from(n - 1).map_err(|_| Error::TooLarge)?)?;
        visit(target, tag, payload)?;
    }
    Ok(())
}

#[cfg(feature = "std")]
pub(crate) fn check_strict_record(tag: [u8; 4], bytes: &[u8]) -> Result<usize, Error> {
    if !super::is_science_tag(&tag) {
        return Ok(0);
    }
    if bytes.first() != Some(&VERSION) && !(matches!(tag, CALB | SMDT) && bytes.first() == Some(&2))
    {
        return Err(Error::Unsupported("unknown science record version"));
    }
    if tag == SMDT {
        let (_, body) = structured_head(bytes)?
            .ok_or(Error::Unsupported("unknown structured metadata format"))?;
        return crate::metadata_pack::unpacked_len(body);
    }
    Ok(0)
}

#[cfg(feature = "std")]
pub(crate) fn validate_plane_records(
    bytes: &[u8],
    descriptors: &[crate::planes::PlaneDescriptor],
    cap: usize,
) -> Result<(), Error> {
    let planes = u32::try_from(descriptors.len()).map_err(|_| Error::TooLarge)?;
    let mut total = bytes.len();
    plane_records(bytes, planes, |_, tag, payload| {
        total = total
            .checked_add(check_strict_record(tag, payload)?)
            .ok_or(Error::TooLarge)?;
        if total > cap {
            return Err(Error::TooLarge);
        }
        Ok(())
    })?;
    if descriptors.is_empty() {
        decode_plane(bytes, 0, 0, None)?;
    }
    for (plane, descriptor) in descriptors.iter().enumerate() {
        decode_plane(bytes, planes, plane as u32, Some(descriptor.sample_type()))?;
    }
    Ok(())
}
