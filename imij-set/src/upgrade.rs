//! Full archive/image migration with an admitted sequence-binding plan.
use crate::reader::Scan;
use crate::wire::{FOOTER, FOOTER_V1, bad};
use crate::{Entry, Reader, SequenceIndexRef, Snapshot, Writer};
use imij::planes::{ReadAt, SliceReader};
use std::{
    collections::BTreeMap,
    io::{self, Write},
};

/// Bounds for archive/image migration. Pixel buffers are never allocated.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct UpgradeOptions {
    /// Input/output encoded-image and nested-archive byte cap and recursion cap.
    pub image_limits: imij::UpgradeLimits,
    /// Maximum distinct referenced sequence indexes across all generations.
    /// The plan retains two bounded archive entries and a new binding per index.
    /// Default 4096; exceeding it fails before output is written.
    pub max_sequence_indexes: usize,
}
impl Default for UpgradeOptions {
    fn default() -> Self {
        Self {
            image_limits: imij::UpgradeLimits::default(),
            max_sequence_indexes: 4096,
        }
    }
}
struct Binding {
    image: Entry,
    index: Entry,
    new: Option<SequenceIndexRef>,
}
fn codec(e: imij::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Upgrade the archive container and all recognized contained IMI images.
///
/// Preserves generations, keys, summaries, ordinary metadata and each segment's
/// hash algorithm. IMI files (including their frames, previews, gain maps and
/// plane tiles) become V4 without decoding pixels. Referenced legacy/current
/// sequence indexes are rebuilt from the upgraded images and rebound. Recognized
/// nested archives are upgraded recursively. Native JPEG/component images are
/// rejected rather than upgraded, including with the optional v2 decoder.
/// Other blobs and image attachments
/// remain opaque and byte-identical. Changed blobs have their cached head-size
/// hint cleared; external offsets and digests must be refreshed by the caller.
///
/// Ordinary opaque blobs stream in 64 KiB chunks. Image/nested-archive rewriting
/// buffers one encoded entry at a time under `options.image_limits`; an entry
/// over the limit fails, never silently stays on the old format. A bounded plan
/// retains sequence-index bindings, not all archive entries or pixels. Indexed
/// images may be read/rebuilt more than once to support forward references and
/// references across generations without retaining their payloads in memory.
/// Reader limits still bound the source generation count.
///
/// Source must remain immutable; sink must be separate, empty, at byte zero.
/// Failure can leave partial output, including earlier completed generations.
/// Publish only after success and your own durability checks. No in-place writes,
/// filesystem replacement or fsync are performed. Successful output is flushed.
pub fn upgrade<S: ReadAt, W: Write>(
    reader: &mut Reader<S>,
    sink: W,
    options: &UpgradeOptions,
) -> io::Result<(W, Snapshot)> {
    run(reader, sink, options, 0)
}
fn run<S: ReadAt, W: Write>(
    reader: &mut Reader<S>,
    mut sink: W,
    options: &UpgradeOptions,
    depth: usize,
) -> io::Result<(W, Snapshot)> {
    if depth > options.image_limits.max_depth {
        return Err(bad("archive upgrade nesting limit"));
    }
    let segments = reader.generation_snapshots()?;
    let mut bindings: BTreeMap<u64, Binding> = BTreeMap::new();
    // Resolve against each source generation, never against a future replacement.
    for segment in &segments {
        let mut scan = Scan::new(segment.clone());
        while let Some(image) = scan.next(reader)? {
            let Some(reference) = SequenceIndexRef::for_upgrade(image.metadata())? else {
                continue;
            };
            let index = reader
                .get_at(segment, reference.key())?
                .ok_or_else(|| bad("missing sequence index"))?;
            if index.len() != reference.encoded_len() || index.checksum() != reference.checksum() {
                return Err(bad("sequence index replacement differs"));
            }
            if let Some(prior) = bindings.get(&index.offset()) {
                if prior.image.len() != image.len() || prior.image.checksum() != image.checksum() {
                    return Err(bad("sequence index bound to different images"));
                }
            } else {
                if bindings.len() >= options.max_sequence_indexes {
                    return Err(bad("archive upgrade sequence plan limit"));
                }
                bindings.insert(
                    index.offset(),
                    Binding {
                        image,
                        index,
                        new: None,
                    },
                );
            }
        }
    }
    // Build new references without holding any image or sequence blob afterward.
    for binding in bindings.values_mut() {
        check_binding(reader, binding)?;
        let image = image(reader, &binding.image, options, depth)?;
        let mut writer =
            Writer::new(io::sink()).with_hash_algorithm(binding.index.checksum().algorithm())?;
        binding.new = Some(write_index(&mut writer, binding, &image)?);
    }
    let mut committed: Option<Snapshot> = None;
    for segment in segments.into_iter().rev() {
        let writer = match &committed {
            None => Writer::new(sink),
            Some(previous) => Writer::append(sink, previous, previous.end())?,
        };
        let mut writer = writer.with_hash_algorithm(segment.algorithm())?;
        let mut scan = Scan::new(segment.clone());
        while let Some(entry) = scan.next(reader)? {
            if let Some(binding) = bindings.get(&entry.offset()) {
                reader.copy_to(&entry, &mut io::sink())?;
                let image = image(reader, &binding.image, options, depth)?;
                let reference = write_index(&mut writer, binding, &image)?;
                if Some(reference) != binding.new {
                    return Err(bad("sequence upgrade identity changed"));
                }
                continue;
            }
            let mut metadata = entry.metadata().clone();
            if let Some(reference) = SequenceIndexRef::for_upgrade(&metadata)? {
                let index = reader
                    .get_at(&segment, reference.key())?
                    .ok_or_else(|| bad("missing sequence index"))?;
                let binding = bindings
                    .get(&index.offset())
                    .ok_or_else(|| bad("missing sequence upgrade plan"))?;
                binding
                    .new
                    .ok_or_else(|| bad("unfinished sequence upgrade plan"))?
                    .write_metadata(&mut metadata)?;
            }
            match kind(reader, &entry)? {
                Kind::Image => {
                    let upgraded = image(reader, &entry, options, depth)?;
                    writer.insert_with_metadata(
                        entry.key(),
                        upgraded.len() as u64,
                        0,
                        entry.summary(),
                        &metadata,
                        &mut upgraded.as_slice(),
                    )?;
                }
                Kind::Archive => {
                    let bytes = reader
                        .read_entry_bounded(&entry, options.image_limits.max_encoded_bytes)?;
                    let mut nested = Reader::open(SliceReader::new(&bytes))?;
                    let output = BoundedVec {
                        bytes: Vec::new(),
                        limit: options.image_limits.max_encoded_bytes,
                    };
                    let (output, _) = run(&mut nested, output, options, depth + 1)?;
                    writer.insert_with_metadata(
                        entry.key(),
                        output.bytes.len() as u64,
                        0,
                        entry.summary(),
                        &metadata,
                        &mut output.bytes.as_slice(),
                    )?;
                }
                Kind::Opaque => {
                    if SequenceIndexRef::for_upgrade(&metadata)?.is_some() {
                        return Err(bad("indexed entry is not an IMI image"));
                    }
                    writer.insert_stream_with_metadata(
                        entry.key(),
                        entry.head_size(),
                        entry.summary(),
                        &metadata,
                        |out| reader.copy_to(&entry, &mut { out }),
                    )?;
                }
            }
        }
        let (output, snapshot) = writer.finish()?;
        sink = output;
        committed = Some(snapshot);
    }
    Ok((sink, committed.ok_or_else(|| bad("empty archive chain"))?))
}
fn image<S: ReadAt>(
    reader: &mut Reader<S>,
    entry: &Entry,
    options: &UpgradeOptions,
    depth: usize,
) -> io::Result<Vec<u8>> {
    let bytes = reader.read_entry_bounded(entry, options.image_limits.max_encoded_bytes)?;
    let mut limits = options.image_limits;
    limits.max_depth = limits
        .max_depth
        .checked_sub(depth)
        .ok_or_else(|| bad("upgrade nesting limit"))?;
    imij::upgrade_image(&bytes, &limits).map_err(codec)
}
fn write_index<W: Write>(
    writer: &mut Writer<W>,
    binding: &Binding,
    image: &[u8],
) -> io::Result<SequenceIndexRef> {
    // This indexes encoded framing; no decoded raster is allocated.
    let mut limits = imij::Limits::DEFAULT;
    limits.max_pixels = u64::MAX;
    limits.max_bytes = u64::MAX;
    writer.insert_sequence_index_for_upgrade(
        binding.index.key(),
        image,
        &limits,
        binding.image.checksum().algorithm(),
        binding.index.summary(),
        binding.index.metadata(),
    )
}
fn check_binding<S: ReadAt>(reader: &mut Reader<S>, binding: &Binding) -> io::Result<()> {
    reader.copy_to(&binding.index, &mut io::sink())?;
    let mut source = reader.entry_source(&binding.index)?;
    let mut magic = [0; 8];
    source.read_exact_at(0, &mut magic).map_err(codec)?;
    if matches!(&magic, b"IMIJSEQ\x03" | b"IMIJSEQ\x04" | b"IMIJSEQ\x05") {
        crate::SequenceIndex::open(source, binding.image.len(), binding.image.checksum())?;
    } else if &magic == b"IMIJSEQ\x01" || &magic == b"IMIJSEQ\x02" {
        let mut header = [0; 40];
        source.read_exact_at(0, &mut header).map_err(codec)?;
        let word = |at| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
        let checksum =
            imij::Digest::from_bytes(imij::HashAlgorithm::Xxh3_64, &header[16..24]).unwrap();
        let width = if magic[7] == 1 { 48 } else { 56 };
        if imij::HashAlgorithm::Xxh3_64
            .digest(&header[..32])
            .as_bytes()
            != &header[32..40]
            || word(8) != binding.image.len()
            || checksum != binding.image.checksum()
            || word(24) == 0
            || word(24).checked_mul(width).and_then(|n| n.checked_add(40))
                != Some(binding.index.len())
        {
            return Err(bad("legacy sequence index image identity/length"));
        }
    } else {
        return Err(bad("unsupported sequence index upgrade version"));
    }
    Ok(())
}
enum Kind {
    Image,
    Archive,
    Opaque,
}
fn kind<S: ReadAt>(reader: &mut Reader<S>, entry: &Entry) -> io::Result<Kind> {
    if entry.len() < 8 {
        return Ok(Kind::Opaque);
    }
    let mut source = reader.entry_source(entry)?;
    let mut magic = [0; 8];
    source.read_exact_at(0, &mut magic).map_err(codec)?;
    if &magic == b"IMIJ\r\n\x87\n" {
        return Ok(Kind::Image);
    }
    for length in [FOOTER, FOOTER_V1] {
        if entry.len() >= length as u64 {
            source
                .read_exact_at(entry.len() - length as u64, &mut magic)
                .map_err(codec)?;
            if Snapshot::footer_len_of(&magic) == Some(length) {
                return Ok(Kind::Archive);
            }
        }
    }
    Ok(Kind::Opaque)
}
struct BoundedVec {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedVec {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(bad("nested archive output limit"));
        }
        self.bytes
            .try_reserve(bytes.len())
            .map_err(|_| io::Error::other("upgrade allocation failed"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
