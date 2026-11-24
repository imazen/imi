//! Positional inspection of the item-local metadata before nested DATA.

use alloc::{string::String, vec::Vec};
use core::ops::Range;

use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::item_metadata::{self, Value};
use crate::parse::{
    chunks::{self, Kind, Rules},
    header::{self, Feature},
    limits::Admission,
    payload::HeadInput,
    positional::Cursor,
};
use crate::planes::ReadAt;
use crate::{ByteReader, Error, Header, ImageInfo};

/// Optional metadata selectors for an indexed item. The filename is always
/// inspected when the item carries kVAL.
#[derive(Clone, Copy, Debug, Default)]
pub struct IndexedMetadataQuery<'a> {
    pub attribute: Option<&'a str>,
    /// Exact schema and instance key. A missing key selects an unnamed record.
    pub structured: Option<(&'a str, Option<&'a str>)>,
}

/// A located kVAL value. The range includes its type and any length prefix.
/// It is relative to the same immutable nested image passed to the probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedAttribute(Range<u64>);

impl IndexedAttribute {
    pub fn range(&self) -> Range<u64> {
        self.0.clone()
    }

    /// Read and validate this value using one positional read.
    pub fn read(&self, source: &mut dyn ReadAt, max_bytes: usize) -> Result<Value, Error> {
        let size = usize::try_from(self.0.end - self.0.start).map_err(|_| Error::TooLarge)?;
        if size > max_bytes {
            return Err(Error::TooLarge);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
        bytes.resize(size, 0);
        source.read_exact_at(self.0.start, &mut bytes)?;
        item_metadata::decode_value(&bytes)
    }
}

/// Item-local information from a selective pre-DATA walk.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct IndexedMetadataSummary {
    pub info: ImageInfo,
    pub orientation: u8,
    pub filename: Option<String>,
    pub attribute: Option<IndexedAttribute>,
    /// Payload range of the matching sMDT chunk, including its identity.
    pub structured: Option<Range<u64>>,
}

struct AdmittedSource<'a> {
    source: &'a mut dyn ReadAt,
    session: &'a DecodeSession,
    length: u64,
}

impl ReadAt for AdmittedSource<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.length)
    }

    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        self.session.v4_input(out.len())?;
        self.session.adapter_work(1 + out.len() as u128)?;
        self.source.read_exact_at(at, out)
    }
}

/// Inspect a nested image without reading DATA or unselected metadata bodies.
///
/// `nested_len` is the bound item's length. The caller may supply already
/// held leading bytes from that same immutable item. Bind the source, length,
/// prefix and enclosing controls to one snapshot before calling. This walk
/// does not verify an enclosing checksum, unpack a body, or scan after DATA.
/// `policy` bounds prefix/header bytes, fetched bytes, chunks and work.
pub fn probe_indexed_nested_summary(
    source: &mut dyn ReadAt,
    nested_len: u64,
    cached_prefix: &[u8],
    outer: &Header,
    query: IndexedMetadataQuery<'_>,
    policy: DecodePolicy,
) -> Result<IndexedMetadataSummary, Error> {
    if cached_prefix.len() as u64 > nested_len {
        return Err(Error::InvalidInput("prefix past indexed image"));
    }
    let session = DecodeSession::new(policy);
    session.v4_input(cached_prefix.len())?;
    let admission = session.v4_models();
    let mut source = AdmittedSource {
        source,
        session: &session,
        length: nested_len,
    };
    let mut input = Cursor::exact(&mut source, nested_len, cached_prefix);
    if input.bytes(header::SIGNATURE.len())? != header::SIGNATURE {
        return Err(Error::NotImij);
    }
    let head = input.varint()?;
    let header_end = input.pos.checked_add(head).ok_or(Error::TooLarge)?;
    let header_limit = session.v4_resume().header_limit();
    if head > header_limit {
        return Err(Error::TooLarge);
    }
    input.pos = 0;
    let bytes = input.vector(header_end, usize::MAX)?;
    let parsed = header::read_versioned(&mut ByteReader::new(&bytes), header_limit)?;
    admission.header(&parsed.header.info, parsed.len)?;
    if !outer.features.covers(parsed.declared) {
        return Err(Error::Corrupt(super::FRAME_UNDECLARED));
    }
    let mut rules = Rules::new(parsed.mixed_pages(), false)
        .with_sequence_items(parsed.declared.contains(Feature::SequenceItems));
    let mut summary = IndexedMetadataSummary {
        info: parsed.header.info,
        orientation: parsed.header.orientation,
        filename: None,
        attribute: None,
        structured: None,
    };
    let mut attributes_seen = false;
    loop {
        admission.chunk_start()?;
        let tag = input.bytes(4)?.try_into().unwrap();
        let length = input.varint()?.checked_sub(1);
        let kind = rules.head(tag, length)?;
        let start = input.pos;
        let end = length
            .map(|len| {
                start
                    .checked_add(len)
                    .filter(|&end| end <= nested_len)
                    .ok_or(Error::Corrupt("chunk past indexed image"))
            })
            .transpose()?;
        match kind {
            Kind::Data => return Ok(summary),
            Kind::Anim | Kind::Frame => return Err(Error::Corrupt("sequence item holds frames")),
            _ => {}
        }
        let end = end.ok_or(Error::Corrupt("open nested metadata"))?;
        input.limit_head(end - start);
        if kind.has_body_rules(&tag) {
            let prefix = input.bytes(chunks::prefix_len(&tag, length) as usize)?;
            rules.identity(tag, prefix)?;
        }
        if item_metadata::is_tag(&tag) {
            if attributes_seen {
                return Err(Error::Corrupt("repeated item metadata"));
            }
            attributes_seen = true;
            read_attributes(&mut input, end, query.attribute, &mut summary)?;
        }
        if &tag == b"sMDT"
            && let Some(selector) = query.structured
            && read_structured(&mut input, selector)?
            && summary.structured.replace(start..end).is_some()
        {
            return Err(Error::Corrupt("repeated structured metadata identity"));
        }
        input.clear_head_limit();
        input.skip_to(end)?;
    }
}

fn read_text(input: &mut Cursor<'_>, max: usize) -> Result<String, Error> {
    let len = usize::try_from(input.varint()?).map_err(|_| Error::TooLarge)?;
    if !(1..=max).contains(&len) {
        return Err(Error::Corrupt("indexed metadata name length"));
    }
    let text = core::str::from_utf8(input.bytes(len)?)
        .map_err(|_| Error::Corrupt("indexed metadata UTF-8"))?;
    item_metadata::name(text, max)?;
    let mut name = String::new();
    name.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    name.push_str(text);
    Ok(name)
}

fn read_attributes(
    input: &mut Cursor<'_>,
    end: u64,
    key: Option<&str>,
    summary: &mut IndexedMetadataSummary,
) -> Result<(), Error> {
    match input.u8()? {
        1 if end - input.pos > item_metadata::MAX_INDEX_METADATA as u64 => {
            return Err(Error::Corrupt("index metadata size"));
        }
        1 | 2 => {}
        _ => return Err(Error::Unsupported("item metadata version")),
    }
    if input.pos == end {
        return Ok(());
    }
    match input.u8()? {
        0 => {}
        1 => summary.filename = Some(read_text(input, 4096)?),
        _ => return Err(Error::Corrupt("index filename flag")),
    }
    let count = input.varint()?;
    if count > 64 {
        return Err(Error::Corrupt("too many index attributes"));
    }
    let Some(key) = key else { return Ok(()) };
    let mut previous: Option<String> = None;
    for _ in 0..count {
        let name = read_text(input, 128)?;
        if previous.as_ref().is_some_and(|last| last >= &name) {
            return Err(Error::Corrupt("index attribute order/duplicate"));
        }
        let start = input.pos;
        let size = match input.u8()? {
            0 | 1 => input.varint()?,
            2..=4 => 8,
            5 => 1,
            _ => return Err(Error::Corrupt("unknown index value type")),
        };
        let next = input
            .pos
            .checked_add(size)
            .filter(|&next| next <= end)
            .ok_or(Error::Corrupt("indexed attribute past map"))?;
        if name == key {
            summary.attribute = Some(IndexedAttribute(start..next));
        }
        previous = Some(name);
        input.skip_to(next)?;
    }
    if input.pos != end {
        return Err(Error::Corrupt("item metadata trailing bytes"));
    }
    Ok(())
}

fn read_structured(
    input: &mut Cursor<'_>,
    (schema, key): (&str, Option<&str>),
) -> Result<bool, Error> {
    let version = input.u8()?;
    if !matches!(version, 1 | 2) || input.u8()? > 2 {
        return Ok(false);
    }
    let name = read_text(input, 64)?;
    let instance = if version == 2 {
        Some(read_text(input, 64)?)
    } else {
        None
    };
    if !name.bytes().all(|b| b.is_ascii_graphic())
        || instance
            .as_ref()
            .is_some_and(|key| !key.bytes().all(|b| b.is_ascii_graphic()))
    {
        return Err(Error::Corrupt("structured metadata identity"));
    }
    Ok(name == schema && instance.as_deref() == key)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::planes::SliceReader;
    use crate::science::{ScienceRecords, StructuredFormat, StructuredMetadata};
    use crate::{Channels, ImageRef, ImiEncoderConfig, Metadata, SampleType};
    use alloc::vec;

    struct Tracked<'a> {
        file: &'a [u8],
        reads: Vec<Range<u64>>,
    }

    impl ReadAt for Tracked<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            panic!("bound length was supplied")
        }

        fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
            self.reads.push(at..at + out.len() as u64);
            SliceReader::new(self.file).read_exact_at(at, out)
        }
    }

    #[test]
    fn browse_skips_a_large_value_and_selection_reads_it_once() {
        let info = ImageInfo::new(8, 8, Channels::Gray, SampleType::U8);
        let mut metadata = Metadata::default();
        let value = Value::Bytes(vec![0xa7; 1 << 20]);
        let mut attributes = item_metadata::ItemMetadata {
            filename: Some("detector".into()),
            ..Default::default()
        };
        attributes.attributes.insert("blob".into(), value.clone());
        attributes
            .attributes
            .insert("last".into(), Value::Bool(true));
        attributes.write_metadata(&mut metadata).unwrap();
        let mut science = ScienceRecords::default();
        science.structured.push(
            StructuredMetadata::new("notes", StructuredFormat::Text, b"note".to_vec())
                .with_instance_key("before"),
        );
        science.structured.push(
            StructuredMetadata::new("notes", StructuredFormat::Text, b"later".to_vec())
                .with_instance_key("after"),
        );
        science.write_metadata(&info, &mut metadata).unwrap();
        let file = ImiEncoderConfig::lossless()
            .encode_samples(ImageRef::packed(&[31; 64], &info), &metadata)
            .unwrap();
        let outer = crate::header(&file).unwrap();
        let mut source = Tracked {
            file: &file,
            reads: Vec::new(),
        };
        let summary = probe_indexed_nested_summary(
            &mut source,
            file.len() as u64,
            &file[..16],
            &outer,
            IndexedMetadataQuery {
                attribute: Some("blob"),
                structured: Some(("notes", Some("after"))),
            },
            DecodePolicy::default().with_max_input_bytes(512),
        )
        .unwrap();
        assert_eq!(summary.info, info);
        assert_eq!(summary.filename.as_deref(), Some("detector"));
        let selected = summary.attribute.unwrap();
        let body = selected.range().end - (1 << 20)..selected.range().end;
        assert!(
            source
                .reads
                .iter()
                .all(|r| r.end <= body.start || r.start >= body.end)
        );
        let structured = summary.structured.unwrap();
        let identity = crate::science::structured_identity(
            &file[structured.start as usize..structured.end as usize],
        )
        .unwrap()
        .unwrap();
        assert_eq!(identity.instance_key, Some("after"));
        let before = source.reads.len();
        assert_eq!(selected.read(&mut source, 1 << 20), Err(Error::TooLarge));
        assert_eq!(source.reads.len(), before);
        assert_eq!(selected.read(&mut source, (1 << 20) + 16).unwrap(), value);
        assert_eq!(&source.reads[before..], &[selected.range()]);
    }
}
