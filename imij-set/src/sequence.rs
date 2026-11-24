//! Optional sequence locations stored as an ordinary archive blob.
use crate::wire::{NONE, SLOT, add, bad, digest_slot, slot_matches};
use crate::{Entry, EntryMetadata, EntrySource, Reader, Summary, Value, Writer};
use imij::planes::ReadAt;
use imij::planes::Rect;
use imij::{Digest, HashAlgorithm, Hasher, ImageInfo, IndexedReference, PixelChecksumStatus};
use std::io::{self, Write};

/// Version 4 reuses the v3 slot sizes with an explicit fRAM offset and
/// a four-byte prefix length in formerly reserved bytes.
const MAGIC: &[u8; 8] = b"IMIJSEQ\x04";
const MAGIC_V3: &[u8; 8] = b"IMIJSEQ\x03";
/// Version 5 stores the remaining bytes of wide reference digests before
/// each record's checksum. The header gives the suffix capacity per record.
const MAGIC_V5: &[u8; 8] = b"IMIJSEQ\x05";
/// magic (8), image length (8), index algorithm id + image digest algorithm
/// id + 6 zero bytes (8), image digest slot (32), image count (8), header
/// digest slot (32).
const HEADER: u64 = 96;
/// fRAM start, nested length, reference ordinal (u64 each), reference digest slot (32),
/// flags: present (1), reference digest algorithm id (1), controls length (4), 2 zero bytes,
/// record digest slot (32).
const RECORD: u64 = 96;
const MAX_SUFFIX: usize = Digest::MAX_RESULT_WIDTH - SLOT;
const MAX_RECORD: usize = RECORD as usize + MAX_SUFFIX;
const KEY: &str = "imij.sequence-index";

fn codec(error: imij::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
fn word(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}
fn record_hash(algo: HashAlgorithm, index: u64, bytes: &[u8]) -> Digest {
    let mut hash = Hasher::new(algo);
    hash.update(&index.to_le_bytes());
    hash.update(bytes);
    hash.finish()
}
fn slot(bytes: &[u8], at: usize) -> [u8; SLOT] {
    bytes[at..at + SLOT].try_into().unwrap()
}

fn reference_digest(algo: HashAlgorithm, prefix: &[u8; SLOT], suffix: &[u8]) -> io::Result<Digest> {
    let width = algo.width();
    let needed = width.saturating_sub(SLOT);
    if needed > suffix.len() || suffix[needed..].iter().any(|&byte| byte != 0) {
        return Err(bad("sequence index reference digest suffix"));
    }
    if needed == 0 {
        return Digest::from_slot(algo, prefix)
            .ok_or_else(|| bad("sequence index reference digest slot"));
    }
    let mut digest = [0; Digest::MAX_RESULT_WIDTH];
    digest[..SLOT].copy_from_slice(prefix);
    digest[SLOT..width].copy_from_slice(&suffix[..needed]);
    Digest::from_bytes(algo, &digest[..width])
        .ok_or_else(|| bad("sequence index reference digest width"))
}

/// Stable binding to a sequence-index blob. A replacement under the same key
/// must retain both length and digest or opening this reference fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceIndexRef {
    key: u64,
    length: u64,
    checksum: Digest,
}
impl SequenceIndexRef {
    pub fn key(&self) -> u64 {
        self.key
    }
    pub fn encoded_len(&self) -> u64 {
        self.length
    }
    pub fn checksum(&self) -> Digest {
        self.checksum
    }

    /// Read the reserved typed index attribute, without fetching the index blob.
    pub fn from_metadata(metadata: &EntryMetadata) -> io::Result<Option<Self>> {
        let Some(value) = metadata.attributes.get(KEY) else {
            return Ok(None);
        };
        let Value::Bytes(bytes) = value else {
            return Err(bad("sequence index attribute type"));
        };
        // The first 18 bytes identify the key, length, and digest algorithm.
        let Some((&id, stored)) = bytes.get(17..).and_then(|rest| rest.split_first()) else {
            return Err(bad("sequence index attribute version/length"));
        };
        let algo =
            HashAlgorithm::from_wire(id).ok_or_else(|| bad("sequence index digest algorithm"))?;
        let checksum = match bytes[0] {
            2 => <&[u8; SLOT]>::try_from(stored)
                .ok()
                .and_then(|slot| Digest::from_slot(algo, slot)),
            3 if algo.width() > SLOT => Digest::from_bytes(algo, stored),
            _ => None,
        }
        .ok_or_else(|| bad("sequence index digest version/width/padding"))?;
        let result = Self {
            key: word(bytes, 1),
            length: word(bytes, 9),
            checksum,
        };
        if result.length < HEADER + RECORD
            || ![RECORD, RECORD + 32, RECORD + 96]
                .into_iter()
                .any(|width| (result.length - HEADER).is_multiple_of(width))
        {
            return Err(bad("sequence index length"));
        }
        Ok(Some(result))
    }

    pub(crate) fn for_upgrade(metadata: &EntryMetadata) -> io::Result<Option<Self>> {
        if let Some(Value::Bytes(bytes)) = metadata.attributes.get(KEY)
            && bytes.len() == 25
            && bytes[0] == 1
        {
            return Ok(Some(Self {
                key: word(bytes, 1),
                length: word(bytes, 9),
                checksum: Digest::from_bytes(HashAlgorithm::Xxh3_64, &bytes[17..25])
                    .ok_or_else(|| bad("legacy sequence digest"))?,
            }));
        }
        Self::from_metadata(metadata)
    }
    /// Replace only the sequence-index attribute, checking ordinary metadata
    /// bounds before changing the caller's value.
    pub fn write_metadata(self, metadata: &mut EntryMetadata) -> io::Result<()> {
        let wide = self.checksum.algorithm().width() > SLOT;
        let mut bytes = vec![if wide { 3 } else { 2 }];
        for n in [self.key, self.length] {
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        bytes.push(self.checksum.algorithm().wire());
        if wide {
            bytes.extend_from_slice(self.checksum.as_bytes());
        } else {
            bytes.extend_from_slice(&digest_slot(self.checksum));
        }
        let mut candidate = metadata.clone();
        candidate.attributes.insert(KEY.into(), Value::Bytes(bytes));
        crate::metadata::encode(&candidate)?;
        *metadata = candidate;
        Ok(())
    }

    /// Resolve against the latest archive generation and bind the index header
    /// to the supplied image entry's length/digest. Only requested index records
    /// are fetched; each carries its own ordinal-bound checksum. Full-blob
    /// verification remains available through [`Reader::copy_to`].
    pub fn open<'a, S: ReadAt>(
        &self,
        archive: &'a mut Reader<S>,
        image: &Entry,
    ) -> io::Result<SequenceIndex<EntrySource<'a, S>>> {
        let entry = archive
            .get(self.key)?
            .ok_or_else(|| bad("missing sequence index"))?;
        if entry.len() != self.length || entry.checksum() != self.checksum {
            return Err(bad("sequence index replacement differs"));
        }
        SequenceIndex::open(archive.entry_source(&entry)?, image.len(), image.checksum())
    }
}

/// One cached location. The image decoder remains authoritative for admission,
/// metadata, reference-raster validation and rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SequenceLocation {
    pub index: u64,
    /// Outer-file byte range; None denotes an empty legacy animation update.
    pub encoded: Option<core::ops::Range<u64>>,
    /// Checked extent of just the selected fRAM prefix, when indexed explicitly.
    pub controls: Option<core::ops::Range<u64>>,
    pub reference_index: Option<u64>,
    /// The reference raster's digest ([`imij::FrameSeekInfo::reference_hash`]).
    pub reference_hash: Option<Digest>,
}

/// Constant-memory positional reader for a cached sequence index. Does not fetch
/// or decode image bytes and is not permission to bypass the IMI file's checks.
pub struct SequenceIndex<S> {
    source: S,
    image_length: u64,
    count: u64,
    algo: HashAlgorithm,
    explicit_controls: bool,
    suffix_len: usize,
}
impl<S: ReadAt> SequenceIndex<S> {
    /// Open a standalone index against an expected immutable image identity.
    /// Digest is the archive entry's full-file digest, not a decoded pixel digest.
    pub fn open(mut source: S, image_length: u64, image_checksum: Digest) -> io::Result<Self> {
        let length = source.len().map_err(codec)?;
        if length < HEADER {
            return Err(bad("truncated sequence index"));
        }
        let mut bytes = [0; HEADER as usize];
        source.read_exact_at(0, &mut bytes).map_err(codec)?;
        let (Some(algo), Some(image_algo)) = (
            HashAlgorithm::from_wire(bytes[16]),
            HashAlgorithm::from_wire(bytes[17]),
        ) else {
            return Err(bad("sequence index digest algorithm"));
        };
        let (explicit_controls, suffix_len) = match &bytes[..8] {
            magic if magic == MAGIC_V3 && bytes[18] == 0 => (false, 0),
            magic if magic == MAGIC && bytes[18] == 0 => (true, 0),
            magic if magic == MAGIC_V5 && matches!(bytes[18], 32 | 96) => {
                (true, usize::from(bytes[18]))
            }
            _ => return Err(bad("sequence index version/reference width")),
        };
        if bytes[19..24] != [0; 5] || !slot_matches(&bytes[64..], algo.digest(&bytes[..64])) {
            return Err(bad("sequence index header/checksum"));
        }
        let count = word(&bytes, 56);
        if word(&bytes, 8) != image_length
            || image_algo != image_checksum.algorithm()
            || !slot_matches(&bytes[24..56], image_checksum)
        {
            return Err(bad("sequence index image identity"));
        }
        if count == 0
            || count
                .checked_mul(RECORD + suffix_len as u64)
                .and_then(|n| n.checked_add(HEADER))
                != Some(length)
        {
            return Err(bad("sequence index count/length"));
        }
        Ok(Self {
            source,
            image_length,
            count,
            algo,
            explicit_controls,
            suffix_len,
        })
    }
    pub fn image_count(&self) -> u64 {
        self.count
    }
    pub fn into_inner(self) -> S {
        self.source
    }

    /// Fetch exactly one fixed-size record. Earlier-only dependencies cannot
    /// form cycles; callers bound their own cache and dependency-chain traversal.
    pub fn get(&mut self, index: u64) -> io::Result<SequenceLocation> {
        if index >= self.count {
            return Err(bad("sequence index ordinal"));
        }
        let record_len = RECORD + self.suffix_len as u64;
        let at = add(
            HEADER,
            index
                .checked_mul(record_len)
                .ok_or_else(|| bad("sequence index offset"))?,
        )?;
        let mut storage = [0; MAX_RECORD];
        let bytes = &mut storage[..record_len as usize];
        self.source.read_exact_at(at, bytes).map_err(codec)?;
        let (record, checksum) = bytes.split_at(64 + self.suffix_len);
        if !slot_matches(checksum, record_hash(self.algo, index, record)) {
            return Err(bad("sequence index record checksum"));
        }
        let (start, len, reference) = (word(record, 0), word(record, 8), word(record, 16));
        let controls_len = if self.explicit_controls {
            if bytes[62..64] != [0; 2] {
                return Err(bad("sequence index reserved"));
            }
            u64::from(u32::from_le_bytes(record[58..62].try_into().unwrap()))
        } else {
            if bytes[58..64] != [0; 6] {
                return Err(bad("sequence index reserved"));
            }
            0
        };
        let digest = match bytes[56] {
            0 => {
                if record[57] != 0
                    || slot(record, 24) != [0; SLOT]
                    || record[64..].iter().any(|&byte| byte != 0)
                {
                    return Err(bad("sequence index flags"));
                }
                None
            }
            1 => {
                let algo = HashAlgorithm::from_wire(bytes[57])
                    .ok_or_else(|| bad("sequence index reference digest algorithm"))?;
                Some(reference_digest(algo, &slot(record, 24), &record[64..])?)
            }
            _ => return Err(bad("sequence index flags")),
        };
        if reference == NONE && digest.is_some() {
            return Err(bad("sequence index flags"));
        }
        let reference_index = if reference == NONE {
            None
        } else {
            if reference >= index {
                return Err(bad("sequence index reference order"));
            }
            Some(reference)
        };
        let (encoded, controls) = if start == NONE {
            if controls_len != 0 {
                return Err(bad("sequence index empty item controls"));
            }
            if len != 0 || reference_index.is_none() {
                return Err(bad("sequence index empty item"));
            }
            (None, None)
        } else {
            if self.explicit_controls && ((index == 0) != (controls_len == 0)) {
                return Err(bad("sequence index controls extent"));
            }
            let nested = add(start, controls_len)?;
            let end = add(nested, len)?;
            if len == 0 || end > self.image_length {
                return Err(bad("sequence index image range"));
            }
            (
                Some(nested..end),
                (controls_len != 0).then_some(start..nested),
            )
        };
        if index == 0 && (encoded != Some(0..self.image_length) || reference_index.is_some()) {
            return Err(bad("sequence index primary image"));
        }
        Ok(SequenceLocation {
            index,
            encoded,
            controls,
            reference_index,
            reference_hash: digest,
        })
    }
}

impl<S: ReadAt> Reader<S> {
    /// Decode a selected unified-sequence item through its indexed byte spans.
    ///
    /// The index must be attached to `image`. A complete nested file is fetched
    /// for each item in the dependency chain; the primary uses the native
    /// random-access region decoder. `max_chain` and `max_compressed_bytes`
    /// bound dependency traversal and each compressed nested file. `limits`
    /// bounds every decoded raster. The outer archive blob digest is not
    /// verified by partial reads, and a supported outer pixel checksum on a
    /// later item is not yet available through this interface; those files
    /// are rejected. The source must keep an immutable identity across reads.
    pub fn decode_indexed_item(
        &mut self,
        image: &Entry,
        ordinal: u64,
        max_chain: usize,
        max_compressed_bytes: usize,
        limits: &imij::Limits,
        threads: usize,
    ) -> io::Result<(ImageInfo, Vec<u8>)> {
        self.decode_indexed_item_inner(
            image,
            ordinal,
            max_chain,
            max_compressed_bytes,
            (limits, false),
            threads,
        )
    }

    /// Decode one indexed item while checking any declared outer file checksum
    /// and every reconstructed reference's stored pixel checksum. With an
    /// outer checksum present, disabled verification fails closed.
    pub fn decode_indexed_item_verified(
        &mut self,
        image: &Entry,
        ordinal: u64,
        max_chain: usize,
        max_compressed_bytes: usize,
        limits: &imij::Limits,
        threads: usize,
    ) -> io::Result<(ImageInfo, Vec<u8>)> {
        self.decode_indexed_item_inner(
            image,
            ordinal,
            max_chain,
            max_compressed_bytes,
            (limits, true),
            threads,
        )
    }

    fn decode_indexed_item_inner(
        &mut self,
        image: &Entry,
        ordinal: u64,
        max_chain: usize,
        max_compressed_bytes: usize,
        (limits, verify_outer): (&imij::Limits, bool),
        threads: usize,
    ) -> io::Result<(ImageInfo, Vec<u8>)> {
        let binding = SequenceIndexRef::from_metadata(image.metadata())?
            .ok_or_else(|| bad("missing sequence index"))?;
        let chain = {
            let mut index = binding.open(self, image)?;
            let mut chain = Vec::new();
            let mut next = Some(ordinal);
            while let Some(i) = next {
                if chain.len() >= max_chain {
                    return Err(bad("sequence reference chain limit"));
                }
                let location = index.get(i)?;
                let preceding_end = if i > 1 && location.controls.is_none() {
                    Some(
                        index
                            .get(i - 1)?
                            .encoded
                            .ok_or_else(|| bad("indexed item follows empty frame"))?
                            .end,
                    )
                } else {
                    None
                };
                next = location.reference_index;
                chain.push((location, preceding_end));
            }
            chain
        };

        let mut primary =
            imij::region::RegionReader::open_with_limits(self.entry_source(image)?, limits)
                .map_err(codec)?;
        let outer = primary.header().clone();
        if ordinal != 0 && !primary.is_unified_sequence() {
            return Err(bad("indexed item requires unified sequence"));
        }
        if !verify_outer
            && ordinal != 0
            && (primary.pixel_checksum_status() == PixelChecksumStatus::Pending
                || primary.has_file_checksum())
        {
            return Err(bad("indexed item needs outer checksum verification"));
        }
        if verify_outer && !limits.verify_checksum {
            return Err(bad(
                "verified indexed access requires checksum verification enabled",
            ));
        }
        let primary_end = primary.primary_end();
        let mut previous: Option<(u64, ImageInfo, Vec<u8>)> = None;
        if chain
            .last()
            .is_some_and(|(location, _)| location.index == 0)
        {
            let info = primary.info().clone();
            let (row, mut pixels) = bounded_raster(&info, limits)?;
            primary
                .decode_region_threads(
                    Rect::new(0, 0, info.width, info.height),
                    &mut pixels,
                    row,
                    threads,
                )
                .map_err(codec)?;
            previous = Some((0, info, pixels));
        }
        for (location, preceding_end) in chain.into_iter().rev() {
            if location.index == 0 {
                continue;
            }
            let span = location
                .encoded
                .ok_or_else(|| bad("empty indexed item needs canvas decoder"))?;
            let controls_start = location
                .controls
                .map_or_else(|| preceding_end.unwrap_or(primary_end), |range| range.start);
            if controls_start < primary_end {
                return Err(bad("indexed fRAM precedes primary end"));
            }
            let controls_len = span
                .start
                .checked_sub(controls_start)
                .and_then(|len| usize::try_from(len).ok())
                .filter(|&len| len <= 64 * 1024)
                .ok_or_else(|| bad("indexed fRAM controls limit"))?;
            let len = usize::try_from(span.end - span.start)
                .ok()
                .filter(|&len| len <= max_compressed_bytes)
                .ok_or_else(|| bad("indexed compressed item limit"))?;
            let total = controls_len
                .checked_add(len)
                .ok_or_else(|| bad("indexed span size"))?;
            let mut source_bytes = Vec::new();
            source_bytes
                .try_reserve_exact(total)
                .map_err(io::Error::other)?;
            source_bytes.resize(total, 0);
            primary
                .read_verified_at(controls_start, &mut source_bytes)
                .map_err(codec)?;
            let (controls, nested) = source_bytes.split_at(controls_len);
            imij::check_indexed_frame_prefix(
                controls,
                outer.version,
                len as u64,
                location.index,
                location.reference_index.zip(location.reference_hash),
            )
            .map_err(codec)?;
            let info = imij::header(nested).map_err(codec)?.info;
            let (row, mut pixels) = bounded_raster(&info, limits)?;
            let reference = match location.reference_index {
                Some(index) => {
                    let (earlier, info, pixels) = previous
                        .as_ref()
                        .ok_or_else(|| bad("missing indexed reference"))?;
                    if *earlier != index {
                        return Err(bad("indexed reference chain differs"));
                    }
                    Some(IndexedReference {
                        index,
                        hash: location
                            .reference_hash
                            .ok_or_else(|| bad("missing reference raster checksum"))?,
                        info,
                        pixels,
                        stride: row,
                    })
                }
                None => None,
            };
            imij::decode_indexed_nested(nested, &outer, reference, &mut pixels, row, threads)
                .map_err(codec)?;
            if verify_outer {
                let ordinal = usize::try_from(location.index).map_err(io::Error::other)?;
                primary
                    .verify_indexed_pixels(ordinal, &info, &pixels, row)
                    .map_err(codec)?;
            }
            previous = Some((location.index, info, pixels));
        }
        let (_, info, pixels) = previous.ok_or_else(|| bad("empty sequence chain"))?;
        Ok((info, pixels))
    }
}

fn bounded_raster(info: &ImageInfo, limits: &imij::Limits) -> io::Result<(usize, Vec<u8>)> {
    let pixels = (info.width as u64)
        .checked_mul(info.height as u64)
        .ok_or_else(|| bad("indexed raster dimensions"))?;
    let row = (info.width as usize)
        .checked_mul(info.bytes_per_pixel())
        .ok_or_else(|| bad("indexed raster row"))?;
    let len = row
        .checked_mul(info.height as usize)
        .ok_or_else(|| bad("indexed raster length"))?;
    if pixels > limits.max_pixels || len as u64 > limits.max_bytes {
        return Err(io::Error::other("indexed raster exceeds decoder limits"));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(io::Error::other)?;
    out.resize(len, 0);
    Ok((row, out))
}

impl<W: Write> Writer<W> {
    /// Insert an optional seek/dependency index as an ordinary opaque blob.
    /// Insert it before its image entry, then attach the returned reference to
    /// that entry's metadata. No index bytes are injected into the IMI file.
    ///
    /// Input is a complete borrowed file. Existing native admission and nested
    /// metadata/checksum parsing apply; pixels are not decoded. Output streams
    /// one record at a time, with no second frame-count-sized index allocation.
    pub fn insert_sequence_index(
        &mut self,
        key: u64,
        image: &[u8],
        limits: &imij::Limits,
    ) -> io::Result<SequenceIndexRef> {
        self.insert_sequence_index_for_upgrade(
            key,
            image,
            limits,
            self.hash_algorithm(),
            Summary::default(),
            &EntryMetadata::default(),
        )
    }
    pub(crate) fn insert_sequence_index_for_upgrade(
        &mut self,
        key: u64,
        image: &[u8],
        limits: &imij::Limits,
        image_algorithm: HashAlgorithm,
        summary: Summary,
        metadata: &EntryMetadata,
    ) -> io::Result<SequenceIndexRef> {
        let mut decoder = imij::FrameDecoder::with_limits(image, limits).map_err(codec)?;
        let count = decoder.frame_count() as u64;
        let mut suffix_len = 0;
        for index in 1..decoder.frame_count() {
            if let Some(digest) = decoder
                .frame_seek_info(index)
                .map_err(codec)?
                .reference_hash
            {
                suffix_len = suffix_len.max(digest.algorithm().width().saturating_sub(SLOT));
            }
        }
        let record_len = RECORD + suffix_len as u64;
        let length = count
            .checked_mul(record_len)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or_else(|| bad("sequence index size"))?;
        let algo = self.hash_algorithm();
        let image_digest = image_algorithm.digest(image);
        let mut header = [0u8; HEADER as usize];
        header[..8].copy_from_slice(if suffix_len == 0 { MAGIC } else { MAGIC_V5 });
        header[8..16].copy_from_slice(&(image.len() as u64).to_le_bytes());
        header[16] = algo.wire();
        header[17] = image_digest.algorithm().wire();
        header[18] = suffix_len as u8;
        header[24..24 + SLOT].copy_from_slice(&digest_slot(image_digest));
        header[56..64].copy_from_slice(&count.to_le_bytes());
        let hash = algo.digest(&header[..64]);
        header[64..].copy_from_slice(&digest_slot(hash));
        let mut all = Hasher::new(algo);
        self.insert_stream_with_metadata(key, 0, summary, metadata, |out| {
            out.write_all(&header)?;
            all.update(&header);
            for index in 0..count {
                // Use the ordinary selector so malformed nested metadata and
                // container controls cannot be promoted into a successful cache.
                decoder.seek_frame(index as usize).map_err(codec)?;
                let point = decoder.frame_seek_info(index as usize).map_err(codec)?;
                let (mut start, len) = point
                    .encoded
                    .map_or((NONE, 0), |r| (r.start, r.end - r.start));
                let controls = match point.controls {
                    Some(range) => {
                        start = range.start;
                        u32::try_from(range.end - range.start)
                            .map_err(|_| bad("sequence index controls length"))?
                    }
                    None => 0,
                };
                let mut storage = [0; MAX_RECORD];
                let bytes = &mut storage[..record_len as usize];
                for (dst, value) in bytes[..24].as_chunks_mut::<8>().0.iter_mut().zip([
                    start,
                    len,
                    point.reference_index.map_or(NONE, |n| n as u64),
                ]) {
                    *dst = value.to_le_bytes();
                }
                if let Some(digest) = point.reference_hash {
                    bytes[24..24 + SLOT].copy_from_slice(&digest_slot(digest));
                    let extra = digest.algorithm().width().saturating_sub(SLOT);
                    if extra != 0 {
                        bytes[64..64 + extra].copy_from_slice(&digest.as_bytes()[SLOT..]);
                    }
                    bytes[56] = 1;
                    bytes[57] = digest.algorithm().wire();
                }
                bytes[58..62].copy_from_slice(&controls.to_le_bytes());
                let (record, checksum) = bytes.split_at_mut(64 + suffix_len);
                checksum.copy_from_slice(&digest_slot(record_hash(algo, index, record)));
                out.write_all(bytes)?;
                all.update(bytes);
            }
            Ok(())
        })?;
        Ok(SequenceIndexRef {
            key,
            length,
            checksum: all.finish(),
        })
    }
}
