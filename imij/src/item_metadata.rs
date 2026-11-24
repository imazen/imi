//! Typed per-image and archive-index attributes with separate byte budgets.
use crate::put_varint as put;
use crate::{ByteReader, Error, Metadata};
#[cfg(all(test, feature = "encode"))]
use alloc::vec;
use alloc::{collections::BTreeMap, string::String, vec::Vec};
fn bad(message: &'static str) -> Error {
    Error::Corrupt(message)
}
struct Cursor<'a>(ByteReader<'a>);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        self.0.bytes(n)
    }
    fn var(&mut self) -> Result<u64, Error> {
        self.0.varint()
    }
    fn byte(&mut self) -> Result<u8, Error> {
        self.0.u8()
    }
    fn fixed(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn end(self) -> Result<(), Error> {
        if self.0.remaining() == 0 {
            Ok(())
        } else {
            Err(bad("item metadata trailing bytes"))
        }
    }
}

pub(crate) const TAG: [u8; 4] = *b"kVAL";
pub(crate) const LEGACY_TAG: [u8; 4] = *b"kVMD";

pub(crate) fn is_tag(tag: &[u8; 4]) -> bool {
    matches!(*tag, TAG | LEGACY_TAG)
}

pub(crate) const MAX_INDEX_METADATA: usize = 16 * 1024;

/// A typed, searchable index value. Floating values must be finite.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    Text(String),
    Bytes(Vec<u8>),
    Unsigned(u64),
    Integer(i64),
    Float(f64),
    Bool(bool),
}

/// Optional application metadata on an image/item or in an archive index.
/// Index values are caller-provided summaries, not authoritative blob metadata.
/// Keys are unique, nonempty UTF-8 strings of at most 128 bytes. Both
/// representations allow up to 64 attributes. An archive index entry is
/// limited to 16 KiB. Image chunks use the image metadata byte policy.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ItemMetadata {
    /// Display/search name, at most 4096 UTF-8 bytes. Never an extraction path.
    pub filename: Option<String>,
    pub attributes: BTreeMap<String, Value>,
}

pub(crate) fn name(s: &str, max: usize) -> Result<(), Error> {
    if s.is_empty() || s.len() > max || s.contains('\0') {
        return Err(bad("index metadata name bounds"));
    }
    Ok(())
}
fn var_size(n: usize) -> usize {
    (usize::BITS - n.leading_zeros()).max(1).div_ceil(7) as usize
}
fn string(out: &mut Vec<u8>, bytes: &[u8]) {
    put(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}
fn read_bytes<'a>(c: &mut Cursor<'a>) -> Result<&'a [u8], Error> {
    let len = usize::try_from(c.var()?).map_err(|_| bad("metadata length"))?;
    c.take(len)
}
fn read_owned_bytes(c: &mut Cursor<'_>) -> Result<Vec<u8>, Error> {
    let value = read_bytes(c)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(value.len())
        .map_err(|_| Error::TooLarge)?;
    bytes.extend_from_slice(value);
    Ok(bytes)
}
fn read_text(c: &mut Cursor<'_>) -> Result<String, Error> {
    String::from_utf8(read_owned_bytes(c)?).map_err(|_| bad("metadata UTF-8"))
}
fn read_value(c: &mut Cursor<'_>) -> Result<Value, Error> {
    Ok(match c.byte()? {
        0 => Value::Text(read_text(c)?),
        1 => Value::Bytes(read_owned_bytes(c)?),
        2 => Value::Unsigned(c.fixed()?),
        3 => Value::Integer(c.fixed()? as i64),
        4 => {
            let v = f64::from_bits(c.fixed()?);
            if !v.is_finite() {
                return Err(bad("nonfinite index value"));
            }
            Value::Float(v)
        }
        5 => match c.byte()? {
            0 => Value::Bool(false),
            1 => Value::Bool(true),
            _ => return Err(bad("index boolean value")),
        },
        _ => return Err(bad("unknown index value type")),
    })
}

pub(crate) fn decode_value(bytes: &[u8]) -> Result<Value, Error> {
    let mut cursor = Cursor(ByteReader::new(bytes));
    let value = read_value(&mut cursor)?;
    cursor.end()?;
    Ok(value)
}
impl ItemMetadata {
    /// Read the unique `kVAL` or legacy `kVMD` chunk under the default image metadata budget.
    pub fn from_metadata(metadata: &Metadata) -> Result<Self, Error> {
        Self::from_metadata_with_byte_limit(
            metadata,
            crate::decode_resources::DEFAULT_METADATA_BYTES,
        )
    }

    /// Read image attributes with a byte limit on the complete chunk payload.
    /// The version-1 archive-compatible body limit also applies to v1 chunks.
    pub fn from_metadata_with_byte_limit(
        metadata: &Metadata,
        max_bytes: usize,
    ) -> Result<Self, Error> {
        let mut chunks = metadata.chunks.iter().filter(|chunk| is_tag(&chunk.tag));
        let Some(chunk) = chunks.next() else {
            return Ok(Self::default());
        };
        if chunks.next().is_some() {
            return Err(bad("repeated item metadata"));
        }
        Self::decode_chunk(&chunk.data, max_bytes)
    }

    pub(crate) fn decode_chunk(bytes: &[u8], max_bytes: usize) -> Result<Self, Error> {
        if bytes.len() > max_bytes {
            return Err(Error::TooLarge);
        }
        match bytes.first() {
            Some(1) => Self::from_bytes(&bytes[1..]),
            Some(2) => Self::decode(&bytes[1..]),
            Some(_) => Err(Error::Unsupported("item metadata version")),
            None => Err(bad("empty item metadata")),
        }
    }
    /// Write image attributes under the default image metadata budget.
    pub fn write_metadata(&self, metadata: &mut Metadata) -> Result<(), Error> {
        self.write_metadata_with_byte_limit(
            metadata,
            crate::decode_resources::DEFAULT_METADATA_BYTES,
        )
    }

    /// Write image attributes under a caller limit on the complete chunk
    /// payload. Small maps use v1 and larger ones use v2.
    pub fn write_metadata_with_byte_limit(
        &self,
        metadata: &mut Metadata,
        max_bytes: usize,
    ) -> Result<(), Error> {
        let mut bytes = self.encode(max_bytes.saturating_sub(1))?;
        let replacement = if bytes.is_empty() {
            None
        } else {
            let version = if bytes.len() <= MAX_INDEX_METADATA {
                1
            } else {
                2
            };
            bytes.try_reserve_exact(1).map_err(|_| Error::TooLarge)?;
            bytes.insert(0, version);
            metadata
                .chunks
                .try_reserve(1)
                .map_err(|_| Error::TooLarge)?;
            Some(crate::Chunk {
                tag: TAG,
                data: bytes,
            })
        };
        metadata.chunks.retain(|chunk| !is_tag(&chunk.tag));
        if let Some(chunk) = replacement {
            metadata.chunks.push(chunk);
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.filename.is_none() && self.attributes.is_empty()
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        self.encode(MAX_INDEX_METADATA)
            .map_err(|error| match error {
                Error::TooLarge => bad("index metadata size"),
                error => error,
            })
    }

    fn encode(&self, max_bytes: usize) -> Result<Vec<u8>, Error> {
        if self.is_empty() {
            return Ok(Vec::new());
        }
        if self.attributes.len() > 64 {
            return Err(bad("too many index attributes"));
        }
        // Bound all owned input before creating a second encoded copy.
        let mut size = 2usize;
        if let Some(filename) = &self.filename {
            name(filename, 4096)?;
            size += filename.len() + var_size(filename.len());
        }
        for (key, value) in &self.attributes {
            name(key, 128)?;
            let value_len = match value {
                Value::Text(v) => v
                    .len()
                    .checked_add(var_size(v.len()))
                    .ok_or_else(|| bad("index metadata size"))?,
                Value::Bytes(v) => v
                    .len()
                    .checked_add(var_size(v.len()))
                    .ok_or_else(|| bad("index metadata size"))?,
                Value::Bool(_) => 1,
                Value::Float(v) if !v.is_finite() => return Err(bad("nonfinite index value")),
                _ => 8,
            };
            size = size
                .checked_add(key.len() + var_size(key.len()) + 1)
                .and_then(|n| n.checked_add(value_len))
                .ok_or_else(|| bad("index metadata size"))?;
            if size > max_bytes {
                return Err(Error::TooLarge);
            }
        }
        if size > max_bytes {
            return Err(Error::TooLarge);
        }
        let mut out = Vec::new();
        out.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
        out.push(u8::from(self.filename.is_some()));
        if let Some(filename) = &self.filename {
            string(&mut out, filename.as_bytes());
        }
        put(&mut out, self.attributes.len() as u64);
        for (key, value) in &self.attributes {
            string(&mut out, key.as_bytes());
            match value {
                Value::Text(v) => {
                    out.push(0);
                    string(&mut out, v.as_bytes());
                }
                Value::Bytes(v) => {
                    out.push(1);
                    string(&mut out, v);
                }
                Value::Unsigned(v) => {
                    out.push(2);
                    out.extend_from_slice(&v.to_le_bytes());
                }
                Value::Integer(v) => {
                    out.push(3);
                    out.extend_from_slice(&v.to_le_bytes());
                }
                Value::Float(v) => {
                    out.push(4);
                    out.extend_from_slice(&v.to_le_bytes());
                }
                Value::Bool(v) => {
                    out.push(5);
                    out.push(u8::from(*v));
                }
            }
        }
        Ok(out)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_INDEX_METADATA {
            return Err(bad("index metadata size"));
        }
        Self::decode(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let mut c = Cursor(ByteReader::new(bytes));
        let filename = match c.byte()? {
            0 => None,
            1 => {
                let s = read_text(&mut c)?;
                name(&s, 4096)?;
                Some(s)
            }
            _ => return Err(bad("index filename flag")),
        };
        let count = c.var()?;
        if count > 64 {
            return Err(bad("too many index attributes"));
        }
        let mut attributes = BTreeMap::new();
        for _ in 0..count {
            let key = read_text(&mut c)?;
            name(&key, 128)?;
            if attributes
                .last_key_value()
                .is_some_and(|(last, _)| last >= &key)
            {
                return Err(bad("index attribute order/duplicate"));
            }
            let value = read_value(&mut c)?;
            attributes.insert(key, value);
        }
        c.end()?;
        Ok(Self {
            filename,
            attributes,
        })
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn image_map_versions_and_limits_are_independent_of_archive_entries() {
        let mut map = ItemMetadata::default();
        let mut metadata = Metadata::default();
        map.attributes
            .insert("k".into(), Value::Bytes(vec![42; MAX_INDEX_METADATA - 7]));
        map.write_metadata(&mut metadata).unwrap();
        assert_eq!(metadata.chunks[0].data[0], 1);
        assert_eq!(metadata.chunks[0].data[1..], map.to_bytes().unwrap());
        assert_eq!(ItemMetadata::from_metadata(&metadata).unwrap(), map);
        map.attributes
            .insert("k".into(), Value::Bytes(vec![42; 128 << 10]));
        assert!(map.to_bytes().is_err());
        let previous = metadata.clone();
        assert_eq!(
            map.write_metadata_with_byte_limit(&mut metadata, 128 << 10),
            Err(Error::TooLarge),
        );
        assert_eq!(metadata.chunks, previous.chunks);
        map.write_metadata(&mut metadata).unwrap();
        let payload = &metadata.chunks[0].data;
        assert_eq!(payload[0], 2);
        assert!(ItemMetadata::from_bytes(&payload[1..]).is_err());
        assert_eq!(
            ItemMetadata::from_metadata_with_byte_limit(&metadata, payload.len() - 1),
            Err(Error::TooLarge)
        );
        assert_eq!(
            ItemMetadata::from_metadata_with_byte_limit(&metadata, payload.len()).unwrap(),
            map
        );
    }

    #[test]
    fn image_attribute_values_round_trip_through_v4_container() {
        let info = crate::ImageInfo::new(8, 8, crate::Channels::Gray, crate::SampleType::U8);
        let pixels = [31; 64];
        for bytes in [32 << 10, 1 << 20] {
            let values: Vec<_> = (0..bytes).map(|i| (i % 251) as u8).collect();
            let mut map = ItemMetadata {
                filename: Some("attribute-witness".into()),
                ..ItemMetadata::default()
            };
            map.attributes.insert("k".into(), Value::Bytes(values));
            let mut metadata = Metadata::default();
            map.write_metadata(&mut metadata).unwrap();
            let file = crate::ImiEncoderConfig::lossless()
                .encode_samples(crate::ImageRef::packed(&pixels, &info), &metadata)
                .unwrap();
            let (_, observed) = crate::probe(&file).unwrap();
            assert_eq!(ItemMetadata::from_metadata(&observed).unwrap(), map);
            assert_eq!(crate::decode(&file).unwrap().1, pixels);

            #[cfg(feature = "std")]
            if let Some(dir) = std::env::var_os("IMI_METADATA_RANGE_FIXTURES") {
                let path = std::path::Path::new(&dir).join(std::format!("{bytes}.imi"));
                std::fs::write(path, &file).unwrap();
            }
        }
    }
    #[test]
    fn typed_attribute_tags_follow_the_container_version() {
        let info = crate::ImageInfo::new(8, 8, crate::Channels::Gray, crate::SampleType::U8);
        let pixels = [31; 64];
        let input = crate::ImageRef::packed(&pixels, &info);
        let config = crate::ImiEncoderConfig::lossless().with_checksum(true);
        let mut map = ItemMetadata::default();
        map.attributes.insert("source".into(), Value::Unsigned(41));
        let mut metadata = Metadata::default();
        map.write_metadata(&mut metadata).unwrap();
        assert_eq!(metadata.chunks[0].tag, TAG);
        let legacy = config.encode_legacy_samples(input, &metadata).unwrap();
        for (file, tag) in [
            (legacy.clone(), LEGACY_TAG),
            (config.encode_samples(input, &metadata).unwrap(), TAG),
            (
                crate::upgrade_image(&legacy, &Default::default()).unwrap(),
                TAG,
            ),
        ] {
            let (_, observed) = crate::probe(&file).unwrap();
            assert_eq!(
                observed.chunks.iter().find(|c| is_tag(&c.tag)).unwrap().tag,
                tag
            );
            assert_eq!(ItemMetadata::from_metadata(&observed).unwrap(), map);
            assert_eq!(crate::decode(&file).unwrap().1, pixels);
        }
        metadata.chunks.push(crate::Chunk {
            tag: LEGACY_TAG,
            data: metadata.chunks[0].data.clone(),
        });
        assert_eq!(
            ItemMetadata::from_metadata(&metadata),
            Err(Error::Corrupt("repeated item metadata"))
        );
    }

    #[test]
    fn metadata_boundary_and_invalid_wire() {
        let mut m = ItemMetadata::default();
        m.attributes
            .insert("k".into(), Value::Bytes(vec![42; MAX_INDEX_METADATA - 7]));
        let bytes = m.to_bytes().unwrap();
        assert_eq!(bytes.len(), MAX_INDEX_METADATA);
        assert_eq!(ItemMetadata::from_bytes(&bytes).unwrap(), m);
        if let Value::Bytes(v) = m.attributes.get_mut("k").unwrap() {
            v.push(0);
        }
        assert!(m.to_bytes().is_err());
        assert!(ItemMetadata::from_bytes(&vec![0; MAX_INDEX_METADATA + 1]).is_err());
        // filename absent, two attributes with the same key and bool value.
        assert!(ItemMetadata::from_bytes(&[0, 2, 1, b'a', 5, 0, 1, b'a', 5, 1]).is_err());
        assert!(ItemMetadata::from_bytes(&[0, 1, 1, 0xff, 5, 0]).is_err());
        assert!(ItemMetadata::from_bytes(&[0, 1, 1, b'a', 5, 2]).is_err());
        assert!(ItemMetadata::from_bytes(&[0, 1, 1, b'a', 99]).is_err());
        assert!(ItemMetadata::from_bytes(&[0, 65]).is_err());
        let mut nonfinite = vec![0, 1, 1, b'a', 4];
        nonfinite.extend_from_slice(&f64::NAN.to_le_bytes());
        assert!(ItemMetadata::from_bytes(&nonfinite).is_err());
        m.attributes.insert("k".into(), Value::Float(f64::INFINITY));
        assert!(m.to_bytes().is_err());
    }
}
