//! Bounded positional traversal shared by strict validation and migration.
//! This checks framing and indexes; only the validator's decoder establishes
//! that entropy payloads reconstruct pixels.
use crate::decode_resources::{DecodePolicy, DecodeSession};
pub(crate) use crate::parse::positional::Cursor;
use crate::parse::{chunks, header, payload::HeadInput, records};
use crate::planes::ReadAt;
use crate::{ByteReader, Error, Hasher};
use alloc::vec::Vec;
use core::ops::Range;

pub(crate) struct RangeSource<'a> {
    pub source: &'a mut dyn ReadAt,
    pub range: Range<u64>,
}
impl ReadAt for RangeSource<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.range.end - self.range.start)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        if at
            .checked_add(out.len() as u64)
            .is_none_or(|end| end > self.range.end - self.range.start)
        {
            return Err(Error::Corrupt("read past image range"));
        }
        self.source.read_exact_at(self.range.start + at, out)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ScanLimits {
    pub file: u64,
    pub head: usize,
    pub metadata: usize,
    pub chunks: usize,
    pub units: usize,
    pub decode: DecodePolicy,
}

#[derive(Clone, Debug)]
pub(crate) struct Chunk {
    pub tag: [u8; 4],
    pub start: u64,
    pub body: Range<u64>,
}
impl Chunk {
    pub fn len(&self) -> u64 {
        self.body.end - self.start
    }
}

pub(crate) struct Scan {
    pub header_bytes: Vec<u8>,
    pub header: header::ParsedHeader,
    pub chunks: Vec<Chunk>,
    pub data_head: u64,
    pub native: bool,
    pub data_units: Vec<(u64, u64)>,
    pub spans: Vec<u64>,
    pub checked_file_checksum: bool,
    v4: Option<crate::v4::traversal::Objects>,
}
impl Scan {
    pub fn chunk(&self, tag: &[u8; 4]) -> Option<&Chunk> {
        self.chunks.iter().find(|c| &c.tag == tag)
    }
    pub fn bytes(
        &self,
        src: &mut dyn ReadAt,
        chunk: &Chunk,
        limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let mut r = Cursor::new(src, chunk.body.end);
        r.pos = chunk.body.start;
        r.vector(chunk.body.end - chunk.body.start, limit)
    }
}

pub(crate) fn scan(source: &mut dyn ReadAt, limits: ScanLimits) -> Result<Scan, Error> {
    let len = source.len()?;
    if len > limits.file {
        return Err(Error::TooLarge);
    }
    let mut r = Cursor::new(source, len);
    if r.bytes(8)? != header::SIGNATURE {
        return Err(Error::NotImij);
    }
    let n = r.varint()?;
    let end = r.pos.checked_add(n).ok_or(Error::TooLarge)?;
    r.pos = 0;
    let header_bytes = r.vector(end, limits.head)?;
    let hp = header::read_header_sized(&mut ByteReader::new(&header_bytes))?;
    header::check_decodable(&hp.header, hp.declared)?;
    records::head_records(&header_bytes[hp.ext_at..], |id, bytes| {
        if id == header::EXT_LOCATOR && header::Locator::parse(bytes).is_none() {
            return Err(Error::Corrupt("malformed DATA locator"));
        }
        Ok(())
    })?;
    let mut rules = chunks::Rules::new(hp.mixed_pages(), true)
        .with_sequence_items(hp.declared.contains(header::Feature::SequenceItems));
    let mut result = Scan {
        header_bytes,
        header: hp,
        chunks: Vec::new(),
        data_head: 0,
        native: false,
        data_units: Vec::new(),
        spans: Vec::new(),
        checked_file_checksum: false,
        v4: None,
    };
    let mut before_data = Vec::new();
    let mut used_metadata = 0usize;
    let mut session = DecodeSession::new(limits.decode);
    while r.pos < len {
        if result.chunks.len() >= limits.chunks {
            return Err(Error::TooLarge);
        }
        let start = r.pos;
        let tag: [u8; 4] = r.bytes(4)?.try_into().unwrap();
        if !tag[2].is_ascii_uppercase() {
            return Err(Error::Corrupt("reserved chunk tag bit"));
        }
        let size = r.varint()?.checked_sub(1);
        let kind = rules.head(tag, size)?;
        let body_start = r.pos;
        let body_end = match size {
            Some(n) => body_start
                .checked_add(n)
                .filter(|&v| v <= len)
                .ok_or(Error::Corrupt("chunk past image range"))?,
            None => len,
        };
        if kind == chunks::Kind::Data && result.header.header.version == 4 {
            let (objects, head, units, spans) = crate::v4::traversal::Objects::scan(
                &mut r,
                &result.header,
                size.map(|_| body_end),
                limits,
                &session,
            )?;
            result.v4 = Some(objects);
            result.data_head = head;
            result.data_units = units;
            result.spans = spans;
        } else if kind == chunks::Kind::Data {
            r.limit_head(limits.head as u64);
            let mut head = session.inspect_payload_head(&result.header, &mut r)?;
            #[cfg(feature = "native-jpeg")]
            {
                result.native = matches!(head, crate::parse::payload::Head::Native(_));
            }
            result.data_head = r.pos - body_start;
            let (map, stripes) = head.units();
            let count = map.count(stripes)?;
            if count > limits.units {
                return Err(Error::TooLarge);
            }
            let mut walk = records::Walk::with_ordinals(
                result.header.header.version,
                head.take_hint(),
                head.unit_ordinals(),
            );
            walk.keep_spans(count)?;
            drop(head);
            r.clear_head_limit();
            for stripe in 0..stripes {
                for group in 0..map.groups() {
                    let (kinds, nkinds) = map.kinds_at(stripe, group);
                    for &expected in &kinds[..nkinds] {
                        loop {
                            let at = r.pos;
                            let id = r.varint()?;
                            let n = r.varint()?;
                            let end = r
                                .pos
                                .checked_add(n)
                                .filter(|&v| v <= body_end)
                                .ok_or(Error::Corrupt("unit past DATA"))?;
                            if result.data_units.len() >= limits.units.saturating_mul(2) {
                                return Err(Error::TooLarge);
                            }
                            result
                                .data_units
                                .try_reserve(1)
                                .map_err(|_| Error::TooLarge)?;
                            result.data_units.push((at, id));
                            let main = walk.head(at, end, id, expected)?;
                            if main && walk.ordinals() && r.varint()? != (walk.used() - 1) as u64 {
                                return Err(Error::Corrupt("unit record ordinal"));
                            }
                            r.skip_to(end)?;
                            if main {
                                break;
                            }
                        }
                    }
                }
            }
            walk.finish()?;
            result.spans = walk.take_spans().unwrap();
            if size.is_some() && r.pos != body_end {
                return Err(Error::Corrupt("trailing DATA bytes"));
            }
            if let Some(locator) = &result.header.locator
                && (locator.prefix != body_start - start + result.data_head
                    || locator.chunks != before_data)
            {
                return Err(Error::Corrupt("DATA locator does not match chunks"));
            }
        } else {
            if !rules.data_seen() {
                before_data.try_reserve(1).map_err(|_| Error::TooLarge)?;
                before_data.push(body_end - start);
            }
            if kind.has_body_rules(&tag) {
                let body = r.vector(
                    body_end - body_start,
                    limits.metadata.saturating_sub(used_metadata),
                )?;
                used_metadata = used_metadata
                    .checked_add(body.len())
                    .ok_or(Error::TooLarge)?;
                rules.body(tag, &body)?;
                if tag == crate::pixelsum::PSUM
                    && matches!(
                        crate::pixelsum::parse(&body)?,
                        crate::pixelsum::Table::Unknown
                    )
                {
                    return Err(Error::Unsupported(
                        "unknown pixel checksum algorithm or version",
                    ));
                }
            } else {
                r.skip_to(body_end)?;
            }
        }
        result.chunks.try_reserve(1).map_err(|_| Error::TooLarge)?;
        result.chunks.push(Chunk {
            tag,
            start,
            body: body_start..r.pos,
        });
    }
    rules.finish()?;
    if let Some(objects) = &result.v4 {
        let tail = if let Some(index) = result.chunk(&chunks::UIDX) {
            if index.body.end != len {
                return Err(Error::Corrupt("V4 directory is not the final chunk"));
            }
            Some((result.bytes(r.source, index, limits.metadata)?, index.len()))
        } else {
            None
        };
        objects.check(tail.as_ref().map(|(b, n)| (b.as_slice(), *n)), &session)?;
    } else if let Some(index) = result.chunk(&chunks::UIDX) {
        let bytes = result.bytes(r.source, index, limits.metadata)?;
        let index_data = records::index_footer(&bytes, index.len())
            .ok_or(Error::Corrupt("invalid unit index footer"))?;
        let data = result.chunk(&chunks::DATA).unwrap();
        let entries = records::index_entries(
            index_data,
            data.body.end - data.body.start,
            result.data_head,
            result.spans.len(),
        )?;
        if entries != result.spans {
            return Err(records::MISMATCH);
        }
    }
    // SPEC 2.7 explicitly permits an absent or nonfinal trailing index:
    // check-tail falls back to walking records. A present index is checked
    // for internal consistency even if a positional reader would ignore it.
    match (result.header.checksum, result.chunk(&crate::checksum::CSUM)) {
        (None, None) => {}
        (Some(ann), Some(chunk)) => {
            let bytes = result.bytes(r.source, chunk, limits.metadata)?;
            let table = crate::checksum::parse_payload(&bytes)?
                .ok_or(Error::Unsupported("unknown file checksum algorithm"))?;
            crate::checksum::same_algorithm(ann.announced.algorithm_id()?, &bytes)?;
            table.check(ann.announced.log2()?, Some(len - chunk.len()))?;
            let mut buffer = [0u8; 65536];
            for piece in 0..table.pieces() {
                let range = table.piece(piece);
                let mut at = range.start;
                let mut hash = Hasher::new(table.algo);
                while at < range.end {
                    let physical = if at < chunk.start {
                        at
                    } else {
                        at + chunk.len()
                    };
                    let boundary = if at < chunk.start {
                        range.end.min(chunk.start)
                    } else {
                        range.end
                    };
                    let n = (boundary - at).min(buffer.len() as u64) as usize;
                    r.source.read_exact_at(physical, &mut buffer[..n])?;
                    hash.update(&buffer[..n]);
                    at += n as u64;
                }
                if !table.matches(piece, &hash.finish()) {
                    return Err(crate::checksum::MISMATCH);
                }
            }
            result.checked_file_checksum = true;
        }
        _ => {
            return Err(Error::Corrupt(
                "file checksum announcement and table differ",
            ));
        }
    }
    Ok(result)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
    fn limits() -> ScanLimits {
        ScanLimits {
            file: 1 << 30,
            head: 64 << 10,
            metadata: 16 << 20,
            chunks: 65536,
            units: 1_000_000,
            decode: DecodePolicy::default(),
        }
    }
    #[test]
    fn traversal_checks_encoder_indexes_and_checksums() {
        let info = ImageInfo::new(71, 43, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.row_bytes() * 43)
            .map(|i| (i * 17 + i / 37) as u8)
            .collect();
        for config in [
            ImiEncoderConfig::default(),
            ImiEncoderConfig::quick(),
            ImiEncoderConfig::lossy(85.0),
        ] {
            let bytes = config
                .encode_samples(
                    ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let mut source = crate::planes::SliceReader::new(&bytes);
            let got = scan(&mut source, limits()).unwrap();
            assert_eq!(got.header.header.info, info);
            assert!(!got.spans.is_empty());
            let short = &bytes[..bytes.len() - 1];
            assert!(scan(&mut crate::planes::SliceReader::new(short), limits()).is_err());
        }
    }
}
