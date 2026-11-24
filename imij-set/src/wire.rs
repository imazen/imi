use crate::EntryMetadata;
use imij::{Digest, HashAlgorithm};
use std::io::{self, ErrorKind};
use std::sync::Arc;

pub(crate) const FANOUT: usize = 128;
pub(crate) const LEAF: usize = 256;
pub(crate) const MAX_PAGE: usize = 64 * 1024;
/// Version 2 footer: fixed 32-byte digest slots and the algorithm id.
pub(crate) const FOOTER: usize = 136;
pub(crate) const MAGIC: &[u8; 8] = b"IMIJSET\x02";
/// Version 1 footer: XXH3-64 digests in u64 fields (read only).
pub(crate) const FOOTER_V1: usize = 80;
pub(crate) const MAGIC_V1: &[u8; 8] = b"IMIJSET\x01";
/// The longest footer any version uses: backward scans read this much.
pub(crate) const FOOTER_MAX: usize = FOOTER;
pub(crate) const NONE: u64 = u64::MAX;

pub(crate) fn bad(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

pub(crate) const SLOT: usize = Digest::MAX_WIDTH;

pub(crate) fn digest_slot(digest: Digest) -> [u8; SLOT] {
    let width = digest.algorithm().width().min(SLOT);
    let mut slot = [0; SLOT];
    slot[..width].copy_from_slice(&digest.as_bytes()[..width]);
    slot
}

/// Check a fixed archive slot against a complete digest. Short digests
/// require canonical zero padding. Wide digests store a prefix.
pub(crate) fn slot_matches(slot: &[u8], digest: Digest) -> bool {
    let width = digest.algorithm().width().min(SLOT);
    slot.len() == SLOT
        && slot[width..].iter().all(|&byte| byte == 0)
        && digest.matches_prefix(&slot[..width])
}

/// Child nodes carry complete digests. A root read from a footer can
/// only have the prefix that fits in its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageHash {
    Full(Digest),
    Prefix {
        algorithm: HashAlgorithm,
        bytes: [u8; SLOT],
    },
}

impl From<Digest> for PageHash {
    fn from(digest: Digest) -> Self {
        Self::Full(digest)
    }
}

impl PageHash {
    fn from_slot(algorithm: HashAlgorithm, bytes: [u8; SLOT]) -> io::Result<Self> {
        if algorithm.width() > SLOT {
            Ok(Self::Prefix { algorithm, bytes })
        } else {
            Digest::from_slot(algorithm, &bytes)
                .map(Self::Full)
                .ok_or_else(|| bad("archive footer root digest padding"))
        }
    }

    fn slot(self) -> [u8; SLOT] {
        match self {
            Self::Full(digest) => digest_slot(digest),
            Self::Prefix { bytes, .. } => bytes,
        }
    }

    pub(crate) fn matches(self, digest: Digest) -> bool {
        match self {
            Self::Full(expected) => expected == digest,
            Self::Prefix { algorithm, bytes } => {
                algorithm == digest.algorithm() && digest.matches_prefix(&bytes)
            }
        }
    }
}
pub(crate) fn add(a: u64, b: u64) -> io::Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| bad("archive offset overflow"))
}
pub(crate) fn put(out: &mut Vec<u8>, mut n: u64) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
pub(crate) struct Cursor<'a>(pub &'a [u8]);
impl<'a> Cursor<'a> {
    pub fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(bad("truncated index"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn fixed(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// An `algo` digest, `algo.width()` bytes.
    pub fn digest(&mut self, algo: HashAlgorithm) -> io::Result<Digest> {
        Digest::from_bytes(algo, self.take(algo.width())?).ok_or_else(|| bad("index digest"))
    }
    /// A 32-byte digest slot: `algo.width()` bytes then zeros.
    pub fn var(&mut self) -> io::Result<u64> {
        let mut n = 0;
        for shift in (0..70).step_by(7) {
            let b = self.byte()?;
            if shift == 63 && b > 1 {
                return Err(bad("index integer overflow"));
            }
            n |= u64::from(b & 127) << shift;
            if b < 128 {
                if shift > 0 && b == 0 {
                    return Err(bad("noncanonical index integer"));
                }
                return Ok(n);
            }
        }
        Err(bad("index integer overflow"))
    }
    pub fn end(self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(bad("index trailing bytes"))
        }
    }
}

/// Optional header summary. Zero dimensions denote an opaque blob.
/// Values describe the entry, not an archive-wide pixel format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Summary {
    pub width: u32,
    pub height: u32,
    /// IMI channel-layout wire identity, not a physical channel count.
    pub channels: u8,
    pub sample: u8,
    pub coding: u8,
    pub features: u64,
    /// Optional cached color code points. Does not override the image's color authority.
    pub cicp: Option<imij::Cicp>,
    /// Number of images in the entry, when known. None means unknown; zero is invalid.
    pub image_count: Option<u64>,
}

/// An immutable indexed byte range. Keys are unsigned 64-bit application IDs.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub(crate) owner: Option<Arc<()>>,
    pub(crate) key: u64,
    pub(crate) offset: u64,
    pub(crate) length: u64,
    pub(crate) head: u64,
    pub(crate) checksum: Digest,
    pub(crate) summary: Summary,
    pub(crate) metadata: EntryMetadata,
}
impl Entry {
    /// The published access profile and split policy, if this entry is governed.
    pub fn access_policy(&self) -> Option<(u64, &str)> {
        crate::policy::declaration(&self.metadata).expect("validated index metadata")
    }

    pub(crate) fn bind(mut self, owner: &Arc<()>) -> Self {
        self.owner = Some(Arc::clone(owner));
        self
    }

    pub fn key(&self) -> u64 {
        self.key
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn head_size(&self) -> u64 {
        self.head
    }
    /// The blob's digest, under the segment's algorithm.
    pub fn checksum(&self) -> Digest {
        self.checksum
    }
    pub fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }
    pub fn summary(&self) -> Summary {
        self.summary
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Node {
    pub max: Option<u64>,
    pub offset: u64,
    pub len: u64,
    pub hash: PageHash,
    pub bounds: Option<Bounds>,
    pub cicp_bounds: Option<CicpBounds>,
    pub filename_bounds: Option<FilenameBounds>,
}
/// Conservative first-16-byte UTF-8 prefix range. Exact equality is checked
/// at the leaf; prefixes that collide only cause extra page reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FilenameBounds {
    pub min: u128,
    pub max: u128,
}
impl FilenameBounds {
    pub fn key(name: Option<&str>) -> u128 {
        let mut prefix = [0; 16];
        if let Some(name) = name {
            let bytes = name.as_bytes();
            let len = bytes.len().min(prefix.len());
            prefix[..len].copy_from_slice(&bytes[..len]);
        }
        u128::from_be_bytes(prefix)
    }
    pub fn from_entries(entries: &[Entry]) -> Option<Self> {
        let first = Self::key(entries.first()?.metadata.filename.as_deref());
        let mut out = Self {
            min: first,
            max: first,
        };
        for entry in &entries[1..] {
            let key = Self::key(entry.metadata.filename.as_deref());
            out.min = out.min.min(key);
            out.max = out.max.max(key);
        }
        Some(out)
    }
    pub fn from_nodes(nodes: &[Node]) -> Option<Self> {
        let mut iter = nodes.iter();
        let mut out = iter.next()?.filename_bounds?;
        for node in iter {
            let next = node.filename_bounds?;
            out.min = out.min.min(next.min);
            out.max = out.max.max(next.max);
        }
        Some(out)
    }
    pub fn write(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.min.to_be_bytes());
        out.extend_from_slice(&self.max.to_be_bytes());
    }
    pub fn read(c: &mut Cursor<'_>) -> io::Result<Self> {
        let min = u128::from_be_bytes(c.take(16)?.try_into().unwrap());
        let max = u128::from_be_bytes(c.take(16)?.try_into().unwrap());
        if min > max {
            return Err(bad("index filename bounds"));
        }
        Ok(Self { min, max })
    }
    pub fn contains(self, key: u128) -> bool {
        self.min <= key && key <= self.max
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CicpBounds {
    pub min: u32,
    pub max: u32,
}
impl CicpBounds {
    pub fn key(cicp: Option<imij::Cicp>) -> u32 {
        cicp.map_or(0, |c| {
            u32::from_le_bytes([
                c.color_primaries,
                c.transfer_characteristics,
                c.matrix_coefficients,
                u8::from(c.full_range),
            ]) + 1
        })
    }
    pub fn from_entries(entries: &[Entry]) -> Option<Self> {
        let first = Self::key(entries.first()?.summary.cicp);
        let mut out = Self {
            min: first,
            max: first,
        };
        for entry in &entries[1..] {
            let key = Self::key(entry.summary.cicp);
            out.min = out.min.min(key);
            out.max = out.max.max(key);
        }
        Some(out)
    }
    pub fn from_nodes(nodes: &[Node]) -> Option<Self> {
        let mut iter = nodes.iter();
        let mut out = iter.next()?.cicp_bounds?;
        for node in iter {
            let next = node.cicp_bounds?;
            out.min = out.min.min(next.min);
            out.max = out.max.max(next.max);
        }
        Some(out)
    }
    pub fn write(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.min.to_le_bytes());
        out.extend_from_slice(&self.max.to_le_bytes());
    }
    pub fn read(c: &mut Cursor<'_>) -> io::Result<Self> {
        let min = u32::from_le_bytes(c.take(4)?.try_into().unwrap());
        let max = u32::from_le_bytes(c.take(4)?.try_into().unwrap());
        if min > max {
            return Err(bad("index CICP bounds"));
        }
        Ok(Self { min, max })
    }
    pub fn contains(self, key: u32) -> bool {
        self.min <= key && key <= self.max
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub width_min: u32,
    pub width_max: u32,
    pub height_min: u32,
    pub height_max: u32,
}
impl Bounds {
    pub fn from_entries(entries: &[Entry]) -> Option<Self> {
        let first = entries.first()?;
        let mut out = Self {
            width_min: first.summary.width,
            width_max: first.summary.width,
            height_min: first.summary.height,
            height_max: first.summary.height,
        };
        for entry in &entries[1..] {
            out.include(entry.summary.width, entry.summary.height);
        }
        Some(out)
    }
    pub fn from_nodes(nodes: &[Node]) -> Option<Self> {
        let mut iter = nodes.iter();
        let mut out = iter.next()?.bounds?;
        for node in iter {
            let next = node.bounds?;
            out.include(next.width_min, next.height_min);
            out.include(next.width_max, next.height_max);
        }
        Some(out)
    }
    fn include(&mut self, width: u32, height: u32) {
        self.width_min = self.width_min.min(width);
        self.width_max = self.width_max.max(width);
        self.height_min = self.height_min.min(height);
        self.height_max = self.height_max.max(height);
    }
    pub fn write(self, out: &mut Vec<u8>) {
        for value in [
            self.width_min,
            self.width_max,
            self.height_min,
            self.height_max,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    pub fn read(c: &mut Cursor<'_>) -> io::Result<Self> {
        let mut next =
            || -> io::Result<u32> { Ok(u32::from_le_bytes(c.take(4)?.try_into().unwrap())) };
        let out = Self {
            width_min: next()?,
            width_max: next()?,
            height_min: next()?,
            height_max: next()?,
        };
        if out.width_min > out.width_max || out.height_min > out.height_max {
            return Err(bad("index dimension bounds"));
        }
        Ok(out)
    }
    pub fn intersects(self, width: (u32, u32), height: (u32, u32)) -> bool {
        self.width_min <= width.1
            && width.0 <= self.width_max
            && self.height_min <= height.1
            && height.0 <= self.height_max
    }
}
impl Node {
    pub fn write(self, out: &mut Vec<u8>) {
        for n in [self.max.expect("child maximum"), self.offset, self.len] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        let PageHash::Full(hash) = self.hash else {
            unreachable!("writer child nodes carry full digests");
        };
        out.extend_from_slice(hash.as_bytes());
    }
    pub fn read(c: &mut Cursor<'_>, algo: HashAlgorithm) -> io::Result<Self> {
        Ok(Self {
            max: Some(c.fixed()?),
            offset: c.fixed()?,
            len: c.fixed()?,
            hash: c.digest(algo)?.into(),
            bounds: None,
            cicp_bounds: None,
            filename_bounds: None,
        })
    }
}

/// A committed archive generation. Obtained by reading or finishing a writer.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub(crate) end: u64,
    pub(crate) previous: u64,
    pub(crate) root: Node,
    pub(crate) entries: u64,
    pub(crate) level: u8,
    pub(crate) generation: u64,
    /// The digest algorithm of this generation's pages and blobs.
    pub(crate) algo: HashAlgorithm,
    /// The footer's own length (80 for version 1, 136 for version 2).
    pub(crate) footer_len: usize,
}
impl Snapshot {
    pub fn end(&self) -> u64 {
        self.end
    }
    /// The digest algorithm of this generation's pages and blobs.
    pub fn algorithm(&self) -> HashAlgorithm {
        self.algo
    }
    /// This footer's length in bytes.
    pub fn footer_len(&self) -> u64 {
        self.footer_len as u64
    }
    /// The committed bytes before this footer.
    pub(crate) fn before(&self) -> u64 {
        self.end - self.footer_len()
    }
    /// Entries in this append segment, including keys that replace older keys.
    pub fn segment_entries(&self) -> u64 {
        self.entries
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// The version 2 footer: magic, end, previous, root offset, root length,
    /// root digest (32-byte slot), entries, level, generation, algorithm id
    /// (padded to 8), then the footer's own digest (32-byte slot) of the
    /// 104 bytes before it.
    pub(crate) fn encode(&self) -> io::Result<Vec<u8>> {
        let mut out = MAGIC.to_vec();
        for n in [self.end, self.previous, self.root.offset, self.root.len] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.extend_from_slice(&self.root.hash.slot());
        for n in [self.entries, u64::from(self.level), self.generation] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.push(self.algo.wire());
        out.extend_from_slice(&[0; 7]);
        out.extend_from_slice(&digest_slot(self.algo.digest(&out)));
        debug_assert_eq!(out.len(), FOOTER);
        Ok(out)
    }
    /// The footer version `bytes` starts with, and its length.
    pub(crate) fn footer_len_of(bytes: &[u8]) -> Option<usize> {
        if bytes.starts_with(MAGIC) {
            Some(FOOTER)
        } else if bytes.starts_with(MAGIC_V1) {
            Some(FOOTER_V1)
        } else {
            None
        }
    }
    /// The footer that ends a `tail` of committed bytes whose last byte is
    /// at `end - 1`.
    pub(crate) fn parse_tail(tail: &[u8], end: u64) -> io::Result<Self> {
        let mut error = bad("archive footer magic/version");
        for len in [FOOTER, FOOTER_V1] {
            if tail.len() >= len && Self::footer_len_of(&tail[tail.len() - len..]) == Some(len) {
                let at = end
                    .checked_sub(len as u64)
                    .ok_or_else(|| bad("footer bounds"))?;
                match Self::parse(&tail[tail.len() - len..], at) {
                    Ok(snapshot) => return Ok(snapshot),
                    Err(candidate_error) => error = candidate_error,
                }
            }
        }
        // A false prefix at end-136 must not hide a valid v1 footer at end-80.
        Err(error)
    }
    pub(crate) fn parse(bytes: &[u8], at: u64) -> io::Result<Self> {
        let footer_len =
            Self::footer_len_of(bytes).ok_or_else(|| bad("archive footer magic/version"))?;
        if bytes.len() != footer_len {
            return Err(bad("archive footer magic/version"));
        }
        let mut c = Cursor(&bytes[8..]);
        let end = c.fixed()?;
        let previous = c.fixed()?;
        let (root_offset, root_len) = (c.fixed()?, c.fixed()?);
        let (root_hash, entries, level, generation, algo, checked) = if footer_len == FOOTER_V1 {
            let root_hash = c.digest(HashAlgorithm::Xxh3_64)?.into();
            let entries = c.fixed()?;
            let level = c.fixed()?;
            let generation = c.fixed()?;
            let hash = c.digest(HashAlgorithm::Xxh3_64)?;
            let checked = HashAlgorithm::Xxh3_64.digest(&bytes[..72]) == hash;
            (
                root_hash,
                entries,
                level,
                generation,
                HashAlgorithm::Xxh3_64,
                checked,
            )
        } else {
            let slot: [u8; Digest::MAX_WIDTH] = c.take(Digest::MAX_WIDTH)?.try_into().unwrap();
            let entries = c.fixed()?;
            let level = c.fixed()?;
            let generation = c.fixed()?;
            let id = c.take(8)?;
            let Some(algo) = HashAlgorithm::from_wire(id[0]) else {
                return Err(io::Error::new(
                    ErrorKind::Unsupported,
                    format!(
                        "archive digest algorithm ID {} is not supported by this build",
                        id[0]
                    ),
                ));
            };
            if id[1..] != [0; 7] {
                return Err(bad("archive footer reserved bytes"));
            }
            let root_hash = PageHash::from_slot(algo, slot)?;
            let hash = c.take(SLOT)?;
            let checked = slot_matches(hash, algo.digest(&bytes[..FOOTER - SLOT]));
            (root_hash, entries, level, generation, algo, checked)
        };
        c.end()?;
        let root = Node {
            max: None,
            offset: root_offset,
            len: root_len,
            hash: root_hash,
            bounds: None,
            cicp_bounds: None,
            filename_bounds: None,
        };
        if end != add(at, footer_len as u64)?
            || level > 15
            || generation == 0
            || (previous != NONE && (previous >= at || previous < FOOTER_V1 as u64))
            || root.len < 4
            || root.len > MAX_PAGE as u64
            || add(root.offset, root.len)? > at
            || !checked
        {
            return Err(bad("archive footer bounds/checksum"));
        }
        Ok(Self {
            end,
            previous,
            root,
            entries,
            level: level as u8,
            generation,
            algo,
            footer_len,
        })
    }
}

pub(crate) fn leaf(entries: &[Entry]) -> Vec<u8> {
    let extended = entries.iter().any(|e| {
        e.summary.cicp.is_some() || e.summary.image_count.is_some() || !e.metadata.is_empty()
    });
    let governed = entries
        .iter()
        .any(|e| e.metadata.attributes.contains_key(crate::policy::PROFILE));
    let kind = if governed {
        crate::policy::LEAF_KIND
    } else if extended {
        2
    } else {
        0
    };
    let mut out = vec![kind, 0];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    // Six basic columns, plus two optional metadata columns; keys/offsets delta-coded.
    let mut cols: [Vec<u8>; 8] = std::array::from_fn(|_| Vec::new());
    let (mut key, mut offset) = (0, 0);
    for e in entries {
        put(&mut cols[0], e.key - key);
        key = e.key;
        put(&mut cols[1], e.offset - offset);
        offset = e.offset;
        put(&mut cols[2], e.length);
        put(&mut cols[3], e.head);
        cols[4].extend_from_slice(e.checksum.as_bytes());
        put(&mut cols[5], u64::from(e.summary.width));
        put(&mut cols[5], u64::from(e.summary.height));
        cols[5].extend_from_slice(&[e.summary.channels, e.summary.sample, e.summary.coding]);
        put(&mut cols[5], e.summary.features);
        if extended {
            cols[6].push(u8::from(e.summary.cicp.is_some()));
            if let Some(c) = e.summary.cicp {
                cols[6].extend_from_slice(&[
                    c.color_primaries,
                    c.transfer_characteristics,
                    c.matrix_coefficients,
                    u8::from(c.full_range),
                ]);
            }
            put(&mut cols[6], e.summary.image_count.unwrap_or(0));
            let metadata = crate::metadata::encode(&e.metadata).expect("writer validated metadata");
            put(&mut cols[7], metadata.len() as u64);
            cols[7].extend_from_slice(&metadata);
        }
    }
    let columns = if extended { 8 } else { 6 };
    // Kind 3 changes only metadata string coding, not blob or summary bytes.
    let compressed = (extended && !governed)
        .then(|| crate::dictionary::compress(&cols[7]))
        .flatten();
    let mut candidate = out.clone();
    candidate[0] = 3;
    if let Some((metadata, table)) = compressed {
        for col in cols[..7].iter().chain([&metadata, &table]) {
            put(&mut candidate, col.len() as u64);
        }
        for col in cols[..7].iter().chain([&metadata, &table]) {
            candidate.extend_from_slice(col);
        }
    }
    for col in &cols[..columns] {
        put(&mut out, col.len() as u64);
    }
    for col in &cols[..columns] {
        out.extend_from_slice(col);
    }
    if candidate.len() > 4 && candidate.len() < out.len() {
        candidate
    } else {
        out
    }
}
pub(crate) fn page_header(bytes: &[u8], level: u8) -> io::Result<(usize, Cursor<'_>)> {
    let mut c = Cursor(bytes);
    let kind = c.byte()?;
    let got = c.byte()?;
    let count = u16::from_le_bytes(c.take(2)?.try_into().unwrap()) as usize;
    if bytes.len() > MAX_PAGE
        || got != level
        || if level == 0 {
            !matches!(kind, 0 | 2 | 3 | crate::policy::LEAF_KIND)
        } else {
            kind != 1 && kind != 4 && kind != 5 && kind != 6
        }
        || count > if level == 0 { LEAF } else { FANOUT }
        || (level > 0 && count == 0)
    {
        return Err(bad("index page kind/count/level"));
    }
    Ok((count, c))
}
pub(crate) fn entries(bytes: &[u8], at: u64, algo: HashAlgorithm) -> io::Result<Vec<Entry>> {
    let (count, mut c) = page_header(bytes, 0)?;
    let extended = bytes[0] >= 2;
    let dictionary_coded = bytes[0] == 3;
    let governed = bytes[0] == crate::policy::LEAF_KIND;
    let mut has_governed_entry = false;
    let mut lengths = vec![
        0;
        if dictionary_coded {
            9
        } else if extended {
            8
        } else {
            6
        }
    ];
    for n in &mut lengths {
        *n = usize::try_from(c.var()?).map_err(|_| bad("column size"))?;
    }
    let mut cols = Vec::new();
    for n in lengths {
        cols.push(Cursor(c.take(n)?));
    }
    c.end()?;
    let dictionary = if dictionary_coded {
        let dictionary = crate::dictionary::table(cols[8].0)?;
        cols[8].0 = &[];
        dictionary
    } else {
        Vec::new()
    };
    // Expanded column bytes retain the original leaf bound: compression never
    // permits an unbounded expansion or a larger logical writer leaf.
    let mut expanded = cols[..if extended { 7 } else { 6 }]
        .iter()
        .map(|c| c.0.len())
        .sum::<usize>();
    let mut out = Vec::with_capacity(count);
    let (mut key, mut offset, mut prior_end) = (0, 0, 0);
    for i in 0..count {
        let dk = cols[0].var()?;
        if i > 0 && dk == 0 {
            return Err(bad("unordered archive keys"));
        }
        key = add(key, dk)?;
        offset = add(offset, cols[1].var()?)?;
        let length = cols[2].var()?;
        let head = cols[3].var()?;
        let checksum = cols[4].digest(algo)?;
        let mut summary = Summary {
            width: u32::try_from(cols[5].var()?).map_err(|_| bad("summary width"))?,
            height: u32::try_from(cols[5].var()?).map_err(|_| bad("summary height"))?,
            channels: cols[5].byte()?,
            sample: cols[5].byte()?,
            coding: cols[5].byte()?,
            features: cols[5].var()?,
            cicp: None,
            image_count: None,
        };
        let metadata = if extended {
            summary.cicp = match cols[6].byte()? {
                0 => None,
                1 => {
                    let b = cols[6].take(4)?;
                    if b[3] > 1 {
                        return Err(bad("CICP range flag"));
                    }
                    Some(imij::Cicp {
                        color_primaries: b[0],
                        transfer_characteristics: b[1],
                        matrix_coefficients: b[2],
                        full_range: b[3] != 0,
                    })
                }
                _ => return Err(bad("CICP presence flag")),
            };
            let count = cols[6].var()?;
            summary.image_count = (count != 0).then_some(count);
            let len = usize::try_from(cols[7].var()?).map_err(|_| bad("index metadata size"))?;
            let encoded = cols[7].take(len)?;
            let raw;
            let bytes = if dictionary_coded {
                raw = crate::dictionary::expand(encoded, &dictionary)?;
                raw.as_slice()
            } else {
                encoded
            };
            expanded = expanded
                .checked_add(bytes.len())
                .ok_or_else(|| bad("expanded leaf size"))?;
            if expanded > MAX_PAGE {
                return Err(bad("expanded leaf size"));
            }
            crate::metadata::decode(bytes)?
        } else {
            EntryMetadata::default()
        };
        if crate::policy::declaration(&metadata)?.is_some() {
            if !governed {
                return Err(bad("governed entry requires a governed index leaf"));
            }
            has_governed_entry = true;
        }
        let end = add(offset, length)?;
        if head > length || end > at || offset < prior_end {
            return Err(bad("entry bounds"));
        }
        prior_end = end;
        out.push(Entry {
            owner: None,
            key,
            offset,
            length,
            head,
            checksum,
            summary,
            metadata,
        });
    }
    for c in cols {
        c.end()?;
    }
    if governed && !has_governed_entry {
        return Err(bad("governed index leaf has no declaration"));
    }
    Ok(out)
}
pub(crate) fn children(
    bytes: &[u8],
    level: u8,
    at: u64,
    algo: HashAlgorithm,
) -> io::Result<Vec<Node>> {
    let (count, mut c) = page_header(bytes, level)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut n = Node::read(&mut c, algo)?;
        if bytes[0] >= 4 {
            n.bounds = Some(Bounds::read(&mut c)?);
        }
        if bytes[0] >= 5 {
            n.cicp_bounds = Some(CicpBounds::read(&mut c)?);
        }
        if bytes[0] == 6 {
            n.filename_bounds = Some(FilenameBounds::read(&mut c)?);
        }
        if n.len < 4
            || n.len > MAX_PAGE as u64
            || add(n.offset, n.len)? > at
            || out.last().is_some_and(|p: &Node| p.max >= n.max)
        {
            return Err(bad("index child bounds/order"));
        }
        out.push(n);
    }
    c.end()?;
    Ok(out)
}

#[cfg(test)]
mod dictionary_tests {
    use super::*;
    use crate::Value;

    fn entry(key: u64, metadata: EntryMetadata) -> Entry {
        Entry {
            owner: None,
            key,
            offset: key,
            length: 1,
            head: 0,
            checksum: Digest::from_bytes(HashAlgorithm::Xxh3_128, &[0; 16]).unwrap(),
            summary: Summary::default(),
            metadata,
        }
    }
    #[test]
    fn dictionary_leaf_roundtrips_and_basic_leaf_bytes_stay_unchanged() {
        let basic = leaf(&[entry(0, EntryMetadata::default())]);
        let mut expected = vec![0, 0, 1, 0, 1, 1, 1, 1, 16, 6, 0, 0, 1, 0];
        expected.extend_from_slice(&[0; 22]);
        assert_eq!(basic, expected);
        let mut meta = EntryMetadata::default();
        meta.filename = Some("variants/photo.imij".into());
        meta.attributes
            .insert("codec-name".into(), Value::Text("zenjpeg".into()));
        let original: Vec<_> = (0..256).map(|key| entry(key, meta.clone())).collect();
        let bytes = leaf(&original);
        assert_eq!(bytes[0], 3);
        assert!(bytes.len() <= MAX_PAGE);
        assert_eq!(
            entries(&bytes, 256, HashAlgorithm::Xxh3_128).unwrap(),
            original
        );
        // A single unique value cannot profit from a page dictionary.
        assert_eq!(leaf(&original[..1])[0], 2);
    }

    #[test]
    fn governed_leaf_requires_its_tag_and_complete_declaration() {
        use crate::policy::{LEAF_KIND, POLICY, PROFILE};

        let algo = HashAlgorithm::Xxh3_128;
        let mut metadata = EntryMetadata::default();
        metadata
            .attributes
            .insert(PROFILE.into(), Value::Unsigned(1));
        metadata
            .attributes
            .insert(POLICY.into(), Value::Text("test revision 1".into()));
        let mut page = leaf(&[
            entry(0, metadata.clone()),
            entry(1, EntryMetadata::default()),
        ]);
        assert_eq!(page[0], LEAF_KIND);
        let parsed = entries(&page, 2, algo).unwrap();
        assert_eq!(parsed[0].access_policy(), Some((1, "test revision 1")));
        assert!(parsed[1].access_policy().is_none());
        page[0] = 2;
        assert!(entries(&page, 2, algo).is_err());

        metadata.attributes.remove(POLICY);
        assert!(entries(&leaf(&[entry(0, metadata)]), 1, algo).is_err());

        let mut metadata = EntryMetadata::default();
        metadata.filename = Some("ordinary".into());
        let mut page = leaf(&[entry(0, metadata)]);
        assert_eq!(page[0], 2);
        page[0] = LEAF_KIND;
        assert!(entries(&page, 1, algo).is_err());
    }
    #[test]
    fn dictionary_pages_cannot_expand_past_a_logical_leaf() {
        let mut dictionary = Vec::new();
        put(&mut dictionary, 1);
        put(&mut dictionary, 4096);
        dictionary.extend_from_slice(&vec![b'x'; 4096]);
        let mut columns: [Vec<u8>; 9] = std::array::from_fn(|_| Vec::new());
        for i in 0..17 {
            put(&mut columns[0], u64::from(i != 0));
            put(&mut columns[1], 0);
            put(&mut columns[2], 0);
            put(&mut columns[3], 0);
            columns[4].extend_from_slice(&[0; 16]);
            columns[5].extend_from_slice(&[0; 6]);
            columns[6].extend_from_slice(&[0; 2]);
            columns[7].extend_from_slice(&[3, 1, 1, 0]); // filename reference, no attributes
        }
        columns[8] = dictionary;
        let mut page = vec![3, 0, 17, 0];
        for column in &columns {
            put(&mut page, column.len() as u64);
        }
        for column in &columns {
            page.extend_from_slice(column);
        }
        assert_eq!(
            entries(&page, 0, HashAlgorithm::Xxh3_128)
                .unwrap_err()
                .to_string(),
            "expanded leaf size"
        );
    }
}
