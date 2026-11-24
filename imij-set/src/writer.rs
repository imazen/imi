use crate::EntryMetadata;
use crate::wire::*;
use imij::{HashAlgorithm, Hasher};
use std::io::{self, Read, Write};

/// Where a segment's index pages go in the file. Both are the same format;
/// every reader reads either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum IndexPlacement {
    /// Each page as soon as it fills, between blobs. Memory holds one leaf
    /// and at most 127 nodes per level.
    #[default]
    Interleaved,
    /// Every page at commit, after the segment's last blob: the leaves in key
    /// order, then each branch level in key order, the root last, then the
    /// footer. One tail read then holds the footer and the branch levels
    /// (about 0.4 bytes per entry), and the leaves of a key range are one
    /// contiguous byte range. Memory holds the segment's encoded leaf pages
    /// until commit (about 26 bytes per entry without metadata).
    AtCommit,
}

/// Append-only archive writer. Input keys must increase strictly within a segment.
/// The sink needs only Write. Memory holds one leaf and at most 127 nodes per level
/// ([`IndexPlacement::Interleaved`], the default).
/// An I/O error poisons this writer; an earlier committed generation stays usable.
pub struct Writer<W> {
    sink: W,
    pos: u64,
    previous: u64,
    generation: u64,
    count: u64,
    last: Option<u64>,
    leaf: Vec<Entry>,
    leaf_budget: usize,
    levels: Vec<Vec<Node>>,
    poisoned: bool,
    placement: IndexPlacement,
    /// [`IndexPlacement::AtCommit`]: encoded leaf pages and their descriptors.
    held: Vec<HeldLeaf>,
    /// The digest algorithm of this segment's blobs, pages and footer.
    algo: HashAlgorithm,
}

/// A leaf page held until commit.
struct HeldLeaf {
    bytes: Vec<u8>,
    max: u64,
    bounds: Option<Bounds>,
    cicp_bounds: Option<CicpBounds>,
    filename_bounds: Option<FilenameBounds>,
}
impl<W: Write> Writer<W> {
    /// The sink must be empty and positioned at byte zero.
    pub fn new(sink: W) -> Self {
        Self::at(sink, 0, NONE, 1)
    }
    /// The sink must be positioned at `start`, at or beyond the committed end.
    /// No old byte is rewritten. Trailing incomplete writes can be left in place.
    pub fn append(sink: W, previous: &Snapshot, start: u64) -> io::Result<Self> {
        if start < previous.end {
            return Err(bad("append would overwrite committed bytes"));
        }
        Ok(Self::at(
            sink,
            start,
            previous.end,
            add(previous.generation, 1)?,
        ))
    }
    fn at(sink: W, pos: u64, previous: u64, generation: u64) -> Self {
        Self {
            sink,
            pos,
            previous,
            generation,
            count: 0,
            last: None,
            leaf: Vec::with_capacity(LEAF),
            leaf_budget: 128,
            levels: Vec::new(),
            poisoned: false,
            placement: IndexPlacement::Interleaved,
            held: Vec::new(),
            algo: HashAlgorithm::DEFAULT,
        }
    }
    /// The digest algorithm of this segment's blobs, pages and footer
    /// ([`HashAlgorithm::DEFAULT`] unless set); before the first entry.
    pub fn with_hash_algorithm(mut self, algorithm: HashAlgorithm) -> io::Result<Self> {
        if self.count != 0 {
            return Err(bad("hash algorithm after the first entry"));
        }
        self.algo = algorithm;
        Ok(self)
    }
    pub fn hash_algorithm(&self) -> HashAlgorithm {
        self.algo
    }
    /// Choose where this segment's index pages go; before the first insert.
    pub fn with_index_placement(mut self, placement: IndexPlacement) -> io::Result<Self> {
        if self.count != 0 {
            return Err(bad("index placement after the first entry"));
        }
        self.placement = placement;
        Ok(self)
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let end = add(self.pos, bytes.len() as u64)?;
        self.sink.write_all(bytes)?;
        self.pos = end;
        Ok(())
    }
    /// Copy exactly `length` bytes, calculating the checksum without holding the blob.
    /// `head_size` is a caller-provided image prefix hint; zero means unknown.
    /// Header summaries are caller declarations, not independently decoded here.
    pub fn insert<R: Read>(
        &mut self,
        key: u64,
        length: u64,
        head_size: u64,
        summary: Summary,
        source: &mut R,
    ) -> io::Result<()> {
        self.insert_with_metadata(
            key,
            length,
            head_size,
            summary,
            &EntryMetadata::default(),
            source,
        )
    }
    /// Copy a blob with optional filename and typed index attributes.
    /// Metadata is bounded and checked before any blob bytes are consumed.
    pub fn insert_with_metadata<R: Read>(
        &mut self,
        key: u64,
        length: u64,
        head_size: u64,
        summary: Summary,
        metadata: &EntryMetadata,
        source: &mut R,
    ) -> io::Result<()> {
        self.insert_stream_with_metadata(key, head_size, summary, metadata, |out| {
            let mut remaining = length;
            let mut buffer = [0u8; 64 * 1024];
            while remaining > 0 {
                let n = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
                source.read_exact(&mut buffer[..n])?;
                out.write_all(&buffer[..n])?;
                remaining -= n as u64;
            }
            Ok(())
        })?;
        Ok(())
    }
    /// Let an encoder stream a blob of initially unknown length directly to the sink.
    /// A failed callback leaves uncommitted bytes and poisons this writer.
    pub fn insert_stream<F>(
        &mut self,
        key: u64,
        head_size: u64,
        summary: Summary,
        write_blob: F,
    ) -> io::Result<u64>
    where
        F: FnOnce(&mut dyn Write) -> io::Result<()>,
    {
        self.insert_stream_with_metadata(
            key,
            head_size,
            summary,
            &EntryMetadata::default(),
            write_blob,
        )
    }

    /// Stream a plaintext nested archive containing the held-out images and scores.
    /// Put the public policy and split revision in `split_policy`.
    pub fn insert_governed_scope<F>(
        &mut self,
        key: u64,
        split_policy: &str,
        write_archive: F,
    ) -> io::Result<u64>
    where
        F: FnOnce(&mut dyn Write) -> io::Result<()>,
    {
        let mut metadata = EntryMetadata::default();
        metadata.attributes.insert(
            crate::policy::PROFILE.into(),
            crate::Value::Unsigned(crate::policy::PLAINTEXT),
        );
        metadata.attributes.insert(
            crate::policy::POLICY.into(),
            crate::Value::Text(split_policy.into()),
        );
        self.insert_stream_with_metadata(key, 0, Summary::default(), &metadata, write_archive)
    }

    /// Stream a blob with optional index metadata; metadata is never injected into its bytes.
    pub fn insert_stream_with_metadata<F>(
        &mut self,
        key: u64,
        head_size: u64,
        summary: Summary,
        metadata: &EntryMetadata,
        write_blob: F,
    ) -> io::Result<u64>
    where
        F: FnOnce(&mut dyn Write) -> io::Result<()>,
    {
        if self.poisoned {
            return Err(bad("writer failed earlier"));
        }
        if summary.image_count == Some(0) {
            return Err(bad("zero image count"));
        }
        crate::policy::declaration(metadata)?;
        let encoded_metadata = crate::metadata::encode(metadata)?;
        // Keep existing leaf placement for narrow algorithms. For wider
        // digests, budget all scalar columns at their largest encoded widths.
        let digest_width = self.algo.width();
        let overhead = if digest_width > SLOT {
            81 + digest_width
        } else {
            96
        };
        let cost = overhead + encoded_metadata.len();
        if self.last.is_some_and(|k| key <= k) {
            return Err(bad("keys must increase within a segment"));
        }
        self.poisoned = true;
        if self.leaf_budget + cost > MAX_PAGE {
            self.flush_leaf()?;
        }
        let offset = self.pos;
        let mut counted = BlobWriter {
            sink: &mut self.sink,
            length: 0,
            hash: Hasher::new(self.algo),
        };
        write_blob(&mut counted)?;
        let length = counted.length;
        let checksum = counted.hash.finish();
        if head_size > length {
            return Err(bad("head size exceeds blob"));
        }
        self.pos = add(offset, length)?;
        self.leaf.push(Entry {
            owner: None,
            key,
            offset,
            length,
            head: head_size,
            checksum,
            summary,
            metadata: metadata.clone(),
        });
        self.leaf_budget += cost;
        self.last = Some(key);
        self.count = add(self.count, 1)?;
        if self.leaf.len() == LEAF {
            self.flush_leaf()?;
        }
        self.poisoned = false;
        Ok(length)
    }
    fn page(
        &mut self,
        bytes: Vec<u8>,
        max: u64,
        bounds: Option<Bounds>,
        cicp_bounds: Option<CicpBounds>,
        filename_bounds: Option<FilenameBounds>,
    ) -> io::Result<Node> {
        if bytes.len() > MAX_PAGE {
            return Err(bad("index page too large"));
        }
        let n = Node {
            max: Some(max),
            offset: self.pos,
            len: bytes.len() as u64,
            hash: self.algo.digest(&bytes).into(),
            bounds,
            cicp_bounds,
            filename_bounds,
        };
        self.write(&bytes)?;
        Ok(n)
    }
    fn push(&mut self, level: usize, node: Node) -> io::Result<()> {
        if level >= 16 {
            return Err(bad("archive index too deep"));
        }
        while self.levels.len() <= level {
            self.levels.push(Vec::with_capacity(FANOUT));
        }
        self.levels[level].push(node);
        if self.levels[level].len() == FANOUT {
            let node = self.flush_level(level)?;
            self.push(level + 1, node)?;
        }
        Ok(())
    }
    fn flush_leaf(&mut self) -> io::Result<()> {
        let max = self.leaf.last().map_or(0, |e| e.key);
        let bounds = Bounds::from_entries(&self.leaf);
        let cicp_bounds = CicpBounds::from_entries(&self.leaf);
        let filename_bounds = FilenameBounds::from_entries(&self.leaf);
        let bytes = leaf(&self.leaf);
        self.leaf.clear();
        self.leaf_budget = 128;
        if self.placement == IndexPlacement::AtCommit {
            if bytes.len() > MAX_PAGE {
                return Err(bad("index page too large"));
            }
            self.held.push(HeldLeaf {
                bytes,
                max,
                bounds,
                cicp_bounds,
                filename_bounds,
            });
            return Ok(());
        }
        let node = self.page(bytes, max, bounds, cicp_bounds, filename_bounds)?;
        self.push(0, node)
    }
    fn flush_level(&mut self, level: usize) -> io::Result<Node> {
        let nodes = std::mem::take(&mut self.levels[level]);
        self.branch(level, nodes)
    }
    /// Writes the branch page at `level + 1` over `nodes`.
    fn branch(&mut self, level: usize, nodes: Vec<Node>) -> io::Result<Node> {
        let bounds = Bounds::from_nodes(&nodes).ok_or_else(|| bad("missing child bounds"))?;
        let cicp_bounds =
            CicpBounds::from_nodes(&nodes).ok_or_else(|| bad("missing child CICP bounds"))?;
        let filename_bounds = FilenameBounds::from_nodes(&nodes)
            .ok_or_else(|| bad("missing child filename bounds"))?;
        let mut bytes = vec![6, (level + 1) as u8];
        bytes.extend_from_slice(&(nodes.len() as u16).to_le_bytes());
        let max = nodes.last().unwrap().max.expect("child maximum");
        for n in nodes {
            n.write(&mut bytes);
            n.bounds.expect("writer child bounds").write(&mut bytes);
            n.cicp_bounds
                .expect("writer child CICP bounds")
                .write(&mut bytes);
            n.filename_bounds
                .expect("writer child filename bounds")
                .write(&mut bytes);
        }
        self.page(
            bytes,
            max,
            Some(bounds),
            Some(cicp_bounds),
            Some(filename_bounds),
        )
    }
    /// Commit one segment. Flushes the sink; durability/fsync is the caller's policy.
    pub fn finish(mut self) -> io::Result<(W, Snapshot)> {
        if self.poisoned {
            return Err(bad("writer failed earlier"));
        }
        if !self.leaf.is_empty() || self.count == 0 {
            self.flush_leaf()?;
        }
        if self.placement == IndexPlacement::AtCommit {
            return self.finish_at_commit();
        }
        let mut level = 0;
        loop {
            let higher = self.levels.iter().skip(level + 1).any(|v| !v.is_empty());
            if !higher && self.levels[level].len() == 1 {
                break;
            }
            if !self.levels[level].is_empty() {
                let n = self.flush_level(level)?;
                self.push(level + 1, n)?;
            }
            level += 1;
        }
        let root = self.levels[level][0];
        let snapshot = Snapshot {
            end: add(self.pos, FOOTER as u64)?,
            previous: self.previous,
            root,
            entries: self.count,
            level: level as u8,
            generation: self.generation,
            algo: self.algo,
            footer_len: FOOTER,
        };
        self.write(&snapshot.encode()?)?;
        self.sink.flush()?;
        Ok((self.sink, snapshot))
    }
    /// [`IndexPlacement::AtCommit`]: the held leaves, then each branch level
    /// in level order (the same chunks of 128 children the interleaved
    /// writer makes), the root last, then the footer.
    fn finish_at_commit(mut self) -> io::Result<(W, Snapshot)> {
        let held = std::mem::take(&mut self.held);
        let mut nodes = Vec::with_capacity(held.len());
        for h in held {
            nodes.push(self.page(h.bytes, h.max, h.bounds, h.cicp_bounds, h.filename_bounds)?);
        }
        let mut level = 0;
        while nodes.len() > 1 {
            if level >= 15 {
                return Err(bad("archive index too deep"));
            }
            let mut parents = Vec::with_capacity(nodes.len().div_ceil(FANOUT));
            let mut rest = nodes.into_iter().peekable();
            while rest.peek().is_some() {
                let chunk: Vec<Node> = rest.by_ref().take(FANOUT).collect();
                parents.push(self.branch(level, chunk)?);
            }
            nodes = parents;
            level += 1;
        }
        let root = nodes[0];
        let snapshot = Snapshot {
            end: add(self.pos, FOOTER as u64)?,
            previous: self.previous,
            root,
            entries: self.count,
            level: level as u8,
            generation: self.generation,
            algo: self.algo,
            footer_len: FOOTER,
        };
        self.write(&snapshot.encode()?)?;
        self.sink.flush()?;
        Ok((self.sink, snapshot))
    }
}
struct BlobWriter<'a, W> {
    sink: &'a mut W,
    length: u64,
    hash: Hasher,
}
impl<W: Write> Write for BlobWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let limit = add(self.length, buf.len() as u64)?;
        let n = self.sink.write(buf)?;
        if n > buf.len() {
            return Err(bad("invalid sink byte count"));
        }
        self.length = limit - (buf.len() - n) as u64;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}
