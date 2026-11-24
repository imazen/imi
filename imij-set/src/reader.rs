use crate::wire::*;
use imij::planes::ReadAt;
use imij::{HashAlgorithm, Hasher};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::sync::Arc;

#[cfg(test)]
#[path = "reader/cache_tests.rs"]
mod cache_tests;

fn source_error(error: imij::Error) -> io::Error {
    io::Error::other(format!("archive source read failed: {error}"))
}

fn allocation_error(_: std::collections::TryReserveError) -> io::Error {
    io::Error::other("archive buffer allocation failed")
}
fn reserved<T>(len: usize) -> io::Result<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(allocation_error)?;
    Ok(out)
}
fn zeroed(len: usize) -> io::Result<Vec<u8>> {
    let mut out = reserved(len)?;
    out.resize(len, 0);
    Ok(out)
}
struct BatchBudget {
    pages: usize,
    bytes: u64,
    range_bytes: u64,
    gap: u64,
}
impl BatchBudget {
    fn read(&mut self, bytes: u64) -> io::Result<()> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| bad("batch read limit"))?;
        Ok(())
    }
}

/// Reader admission limits. Batch limits bound work and allocation dimensions;
/// they are not an allocator/RSS budget. Each parsed page and entry metadata
/// also obey the wire-format bounds.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Limits {
    pub max_segments: usize,
    /// Index pages to read in one batch whose byte ranges are at most this
    /// far apart are read as one range, gap included: one request instead of
    /// several on a range-per-request source.
    pub page_gap: u64,
    /// Maximum keys in one `get_many`, including duplicates.
    pub max_batch_keys: usize,
    /// Maximum decoded pages across all levels and segments of one `get_many`.
    pub max_batch_pages: usize,
    /// Maximum fetched bytes, including merged gaps and older footers, per lookup.
    pub max_batch_read_bytes: u64,
    /// Maximum bytes in a merged lookup range. A larger gap never overrides this cap.
    pub max_merged_range_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_segments: 64,
            page_gap: MAX_PAGE as u64,
            max_batch_keys: 4096,
            max_batch_pages: 1024,
            max_batch_read_bytes: 32 * 1024 * 1024,
            max_merged_range_bytes: 1024 * 1024,
        }
    }
}

/// A snapshot reader over positional I/O, including remote range sources.
/// The source must remain byte-identical for the reader lifetime. Remote sources
/// must pin an immutable object version or use conditional reads (for example,
/// an ETag with If-Match) and fail if it changes; a footer is not a remote lock.
/// Opening fetches 64 KiB from the tail ([`Reader::open_with_tail`] chooses
/// more); the top index normally shares it, and with
/// [`crate::IndexPlacement::AtCommit`] every branch level of a segment up to
/// about 150,000 entries does.
pub struct Reader<S> {
    source: S,
    snapshot: Snapshot,
    owner: Arc<()>,
    audit_receipt: Option<String>,
    tail_start: u64,
    tail: Vec<u8>,
    limits: Limits,
    index_cache: VecDeque<CachedIndex>,
    index_cache_bytes: usize,
}

// Retain fetched buffers, including merged batches, without copying raw pages.
// The opening tail is separate. This bounds encoded capacity, not process RSS.
const INDEX_CACHE_BYTES: usize = 1024 * 1024;
const INDEX_CACHE_RANGES: usize = 16;
struct CachedIndex {
    offset: u64,
    bytes: Vec<u8>,
}
impl<S: ReadAt> Reader<S> {
    pub fn open(source: S) -> io::Result<Self> {
        Self::open_with_tail(source, MAX_PAGE as u64)
    }
    /// [`Self::open`] reading the last `tail_bytes` (at least 64 KiB) in its
    /// one read. Index pages inside the tail cost no further read; this
    /// memory is held while the reader lives.
    pub fn open_with_tail(mut source: S, tail_bytes: u64) -> io::Result<Self> {
        let len = source.len().map_err(source_error)?;
        if len < FOOTER_V1 as u64 {
            return Err(bad("archive has no committed footer"));
        }
        let want = tail_bytes.max(MAX_PAGE as u64);
        let start = len.saturating_sub(want);
        if usize::try_from(len - start).is_err() {
            return Err(bad("tail read too large"));
        }
        let mut tail =
            zeroed(usize::try_from(len - start).map_err(|_| bad("tail read too large"))?)?;
        source
            .read_exact_at(start, &mut tail)
            .map_err(source_error)?;
        let snapshot = Snapshot::parse_tail(&tail, len)?;
        let mut out = Self {
            source,
            snapshot,
            owner: Arc::new(()),
            audit_receipt: None,
            tail_start: start,
            tail,
            limits: Limits::default(),
            index_cache: VecDeque::new(),
            index_cache_bytes: 0,
        };
        out.page(&out.snapshot.clone(), out.snapshot.root, out.snapshot.level)?;
        Ok(out)
    }
    /// Find the last valid footer within an explicit tail scan budget after a torn append.
    /// No old bytes are changed. Callers may increase the budget or retain a known snapshot.
    pub fn recover(mut source: S, max_scan_bytes: u64) -> io::Result<Self> {
        let len = source.len().map_err(source_error)?;
        let lower = len.saturating_sub(max_scan_bytes);
        let mut end = len;
        while end >= FOOTER_V1 as u64 && end > lower {
            let start = end.saturating_sub(MAX_PAGE as u64).max(lower);
            let mut bytes = vec![0; (end - start) as usize];
            source
                .read_exact_at(start, &mut bytes)
                .map_err(source_error)?;
            for i in (0..=bytes.len().saturating_sub(FOOTER_V1)).rev() {
                let Some(footer_len) = bytes.get(i..).and_then(Snapshot::footer_len_of) else {
                    continue;
                };
                let Some(footer) = bytes.get(i..i + footer_len) else {
                    continue;
                };
                if let Ok(snapshot) = Snapshot::parse(footer, start + i as u64) {
                    let root =
                        match read_page(&mut source, &snapshot, snapshot.root, snapshot.level) {
                            Ok((bytes, _)) => bytes,
                            Err(error) if error.kind() == io::ErrorKind::InvalidData => continue,
                            Err(error) => return Err(error),
                        };
                    // Keep the checked top page; arbitrary damaged bytes are not retained.
                    return Ok(Self {
                        source,
                        owner: Arc::new(()),
                        audit_receipt: None,
                        tail_start: snapshot.root.offset,
                        tail: root,
                        snapshot,
                        limits: Limits::default(),
                        index_cache: VecDeque::new(),
                        index_cache_bytes: 0,
                    });
                }
            }
            if start == lower {
                break;
            }
            end = add(start, (FOOTER_MAX - 1) as u64)?;
        }
        Err(bad("no committed footer in recovery budget"))
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
    pub fn into_inner(self) -> S {
        self.source
    }
    fn cached_index(&self, node: Node) -> Option<&[u8]> {
        self.index_cache.iter().find_map(|range| {
            let start = usize::try_from(node.offset.checked_sub(range.offset)?).ok()?;
            let end = start.checked_add(usize::try_from(node.len).ok()?)?;
            range.bytes.get(start..end)
        })
    }
    fn touch_index(&mut self, node: Node) {
        let Some(i) = self.index_cache.iter().position(|range| {
            node.offset >= range.offset
                && node.offset - range.offset <= range.bytes.len() as u64
                && node.len <= range.bytes.len() as u64 - (node.offset - range.offset)
        }) else {
            return;
        };
        if i + 1 != self.index_cache.len() {
            let range = self.index_cache.remove(i).unwrap();
            self.index_cache.push_back(range);
        }
    }
    fn retain_index(&mut self, offset: u64, bytes: Vec<u8>) -> io::Result<()> {
        let capacity = bytes.capacity();
        if capacity > INDEX_CACHE_BYTES {
            return Ok(());
        }
        while self.index_cache.len() == INDEX_CACHE_RANGES
            || self.index_cache_bytes + capacity > INDEX_CACHE_BYTES
        {
            let range = self.index_cache.pop_front().unwrap();
            self.index_cache_bytes -= range.bytes.capacity();
        }
        self.index_cache.try_reserve(1).map_err(allocation_error)?;
        self.index_cache.push_back(CachedIndex { offset, bytes });
        self.index_cache_bytes += capacity;
        Ok(())
    }
    fn page(&mut self, segment: &Snapshot, node: Node, level: u8) -> io::Result<Page> {
        let end = add(node.offset, node.len)?;
        if node.offset >= self.tail_start && end <= add(self.tail_start, self.tail.len() as u64)? {
            validate_page(
                &self.tail
                    [(node.offset - self.tail_start) as usize..(end - self.tail_start) as usize],
                node,
                level,
                segment.before(),
                segment.algo,
            )
        } else if let Some(bytes) = self.cached_index(node) {
            let page = validate_page(bytes, node, level, segment.before(), segment.algo)?;
            self.touch_index(node);
            Ok(page)
        } else {
            let (bytes, page) = read_page(&mut self.source, segment, node, level)?;
            self.retain_index(node.offset, bytes)?;
            Ok(page)
        }
    }
    fn previous(&mut self, current: &Snapshot) -> io::Result<Option<Snapshot>> {
        if current.previous == NONE {
            return Ok(None);
        }
        // Each generation selects its own footer grammar. A v2 append can
        // point to an 80-byte v1 footer, including a very small first segment.
        let length = current.previous.min(FOOTER_MAX as u64) as usize;
        let at = current.previous - length as u64;
        let mut bytes = [0; FOOTER_MAX];
        self.source
            .read_exact_at(at, &mut bytes[..length])
            .map_err(source_error)?;
        let prev = Snapshot::parse_tail(&bytes[..length], current.previous)?;
        if prev.generation.checked_add(1) != Some(current.generation) {
            return Err(bad("generation chain"));
        }
        Ok(Some(prev))
    }
    pub(crate) fn generation_snapshots(&mut self) -> io::Result<Vec<Snapshot>> {
        let mut segments = Vec::new();
        let mut segment = self.snapshot.clone();
        loop {
            if segments.len() >= self.limits.max_segments {
                return Err(bad("segment limit"));
            }
            segments.try_reserve(1).map_err(allocation_error)?;
            segments.push(segment.clone());
            match self.previous(&segment)? {
                Some(previous) => segment = previous,
                None => break,
            }
        }
        if segment.generation != 1 {
            return Err(bad("first archive generation must be one"));
        }
        Ok(segments)
    }
    pub(crate) fn upgrade_container_into<W: Write>(
        &mut self,
        mut sink: W,
    ) -> io::Result<(W, Snapshot)> {
        let segments = self.generation_snapshots()?;
        let mut committed: Option<Snapshot> = None;
        for segment in segments.into_iter().rev() {
            let writer = match &committed {
                None => crate::Writer::new(sink),
                Some(previous) => crate::Writer::append(sink, previous, previous.end())?,
            };
            let mut writer = writer.with_hash_algorithm(segment.algo)?;
            let mut scan = Scan::new(segment);
            while let Some(entry) = scan.next(self)? {
                writer.insert_stream_with_metadata(
                    entry.key(),
                    entry.head_size(),
                    entry.summary(),
                    entry.metadata(),
                    |out| self.copy_to(&entry, &mut { out }),
                )?;
            }
            let (output, snapshot) = writer.finish()?;
            sink = output;
            committed = Some(snapshot);
        }
        // The admitted chain always contains the reader's current snapshot.
        Ok((
            sink,
            committed.ok_or_else(|| bad("empty generation chain"))?,
        ))
    }
    /// Latest key wins across segments. Each segment uses a bounded-depth index descent.
    pub fn get(&mut self, key: u64) -> io::Result<Option<Entry>> {
        self.get_at(&self.snapshot.clone(), key)
    }
    pub(crate) fn get_at(&mut self, snapshot: &Snapshot, key: u64) -> io::Result<Option<Entry>> {
        let mut segment = snapshot.clone();
        for _ in 0..self.limits.max_segments {
            let mut node = segment.root;
            let mut level = segment.level;
            loop {
                match self.page(&segment, node, level)? {
                    Page::Leaf(mut entries) => {
                        if let Ok(i) = entries.binary_search_by_key(&key, |e| e.key) {
                            return Ok(Some(entries.swap_remove(i).bind(&self.owner)));
                        }
                        break;
                    }
                    Page::Branch(children) => {
                        let Some(child) = children.into_iter().find(|n| n.max >= Some(key)) else {
                            break;
                        };
                        node = child;
                        level -= 1;
                    }
                }
            }
            match self.previous(&segment)? {
                Some(prev) => segment = prev,
                None => return Ok(None),
            }
        }
        Err(bad(
            "segment limit; compact archive or raise explicit reader limit",
        ))
    }
    /// [`Self::get`] for many keys at once, results in the order given
    /// (duplicates allowed). Each append segment is descended one index
    /// level at a time for every key still unresolved there, reading that
    /// level's distinct pages in one `read_many_at` batch; pages inside the
    /// tail read at open cost no read. Latest key wins across segments, as
    /// with `get`: a key a newer segment lacks continues to older ones.
    pub fn get_many(&mut self, keys: &[u64]) -> io::Result<Vec<Option<Entry>>> {
        if keys.len() > self.limits.max_batch_keys {
            return Err(bad("batch key limit"));
        }
        let mut out = reserved(keys.len())?;
        out.resize_with(keys.len(), || None);
        let mut pending = reserved(keys.len())?;
        pending.extend(0..keys.len());
        let mut budget = BatchBudget {
            pages: self.limits.max_batch_pages,
            bytes: self.limits.max_batch_read_bytes,
            range_bytes: self.limits.max_merged_range_bytes,
            gap: self.limits.page_gap,
        };
        if keys.is_empty() {
            return Ok(out);
        }
        let mut segment = self.snapshot.clone();
        for _ in 0..self.limits.max_segments {
            if pending.is_empty() {
                return Ok(out);
            }
            let mut unresolved = reserved(keys.len())?;
            let mut frontier = reserved(1)?;
            frontier.push((segment.root, pending));
            let mut level = segment.level;
            loop {
                let mut nodes = reserved(frontier.len())?;
                nodes.extend(frontier.iter().map(|f| f.0));
                let pages = self.pages(&segment, &nodes, level, &mut budget)?;
                let mut next = reserved(keys.len())?;
                for ((_, wanted), page) in frontier.into_iter().zip(pages) {
                    match page {
                        Page::Leaf(entries) => {
                            for i in wanted {
                                match entries.binary_search_by_key(&keys[i], |e| e.key) {
                                    Ok(j) => out[i] = Some(entries[j].clone().bind(&self.owner)),
                                    Err(_) => unresolved.push(i),
                                }
                            }
                        }
                        Page::Branch(children) => {
                            let mut groups = reserved(children.len())?;
                            groups.resize_with(children.len(), Vec::new);
                            for i in wanted {
                                match children.iter().position(|n| n.max >= Some(keys[i])) {
                                    Some(c) => {
                                        groups[c].try_reserve(1).map_err(allocation_error)?;
                                        groups[c].push(i);
                                    }
                                    None => unresolved.push(i),
                                }
                            }
                            next.extend(
                                children
                                    .into_iter()
                                    .zip(groups)
                                    .filter(|(_, g)| !g.is_empty()),
                            );
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                frontier = next;
                level = level
                    .checked_sub(1)
                    .ok_or_else(|| bad("index level below leaves"))?;
            }
            pending = unresolved;
            if pending.is_empty() {
                return Ok(out);
            }
            if segment.previous != NONE {
                budget.read(segment.previous.min(FOOTER_MAX as u64))?;
            }
            match self.previous(&segment)? {
                Some(prev) => segment = prev,
                None => return Ok(out),
            }
        }
        Err(bad(
            "segment limit; compact archive or raise explicit reader limit",
        ))
    }
    /// Index pages `nodes` at `level`: those in the open tail from it, the
    /// rest in one `read_many_at` batch, each validated.
    fn pages(
        &mut self,
        segment: &Snapshot,
        nodes: &[Node],
        level: u8,
        budget: &mut BatchBudget,
    ) -> io::Result<Vec<Page>> {
        budget.pages = budget
            .pages
            .checked_sub(nodes.len())
            .ok_or_else(|| bad("batch page limit"))?;
        let before = segment.before();
        let tail_end = add(self.tail_start, self.tail.len() as u64)?;
        let mut want = reserved(nodes.len())?;
        for node in nodes {
            let end = add(node.offset, node.len)?;
            if node.len < 4 || node.len > MAX_PAGE as u64 || end > before {
                return Err(bad("page bounds"));
            }
            if !(node.offset >= self.tail_start && end <= tail_end)
                && self.cached_index(*node).is_none()
            {
                if node.len > budget.range_bytes {
                    return Err(bad("merged range limit smaller than page"));
                }
                want.push((node.offset, end));
            }
        }
        want.sort_unstable();
        let mut runs: Vec<(u64, u64)> = reserved(want.len())?;
        for (offset, end) in want {
            match runs.last_mut() {
                Some(r)
                    if offset <= r.1.saturating_add(budget.gap)
                        && end.max(r.1) - r.0 <= budget.range_bytes =>
                {
                    r.1 = r.1.max(end);
                }
                _ => runs.push((offset, end)),
            }
        }
        // Admit the entire level before allocating or issuing any range read.
        let bytes = runs.iter().try_fold(0u64, |sum, r| add(sum, r.1 - r.0))?;
        budget.read(bytes)?;
        let mut bufs = reserved(runs.len())?;
        for r in &runs {
            bufs.push(zeroed(
                usize::try_from(r.1 - r.0).map_err(|_| bad("range size"))?,
            )?);
        }
        if !runs.is_empty() {
            let mut requests = reserved(runs.len())?;
            requests.extend(
                runs.iter()
                    .zip(&mut bufs)
                    .map(|(r, b)| (r.0, b.as_mut_slice())),
            );
            self.source
                .read_many_at(&mut requests)
                .map_err(source_error)?;
        }
        let mut pages = reserved(nodes.len())?;
        for node in nodes {
            let end = add(node.offset, node.len)?;
            let bytes = if node.offset >= self.tail_start && end <= tail_end {
                let o = (node.offset - self.tail_start) as usize;
                &self.tail[o..o + node.len as usize]
            } else if let Some(bytes) = self.cached_index(*node) {
                bytes
            } else {
                let i = runs
                    .iter()
                    .position(|r| r.0 <= node.offset && end <= r.1)
                    .ok_or_else(|| bad("missing page range"))?;
                let r = runs.get(i).ok_or_else(|| bad("missing page range"))?;
                let o = usize::try_from(
                    node.offset
                        .checked_sub(r.0)
                        .ok_or_else(|| bad("page range"))?,
                )
                .map_err(|_| bad("page offset"))?;
                &bufs[i][o..o + node.len as usize]
            };
            // Parse directly from the merged range, without a second raw-page buffer.
            pages.push(validate_page(bytes, *node, level, before, segment.algo)?);
        }
        // All requested bindings validate before any buffer can be evicted.
        for node in nodes {
            self.touch_index(*node);
        }
        for (range, bytes) in runs.iter().zip(bufs) {
            self.retain_index(range.0, bytes)?;
        }
        Ok(pages)
    }
    /// Begin sorted, latest-wins listing without reading blob payloads.
    /// Memory holds up to 16 decoded prefetched leaves, one current leaf and a bounded
    /// index stack per append segment.
    pub fn listing(&mut self) -> io::Result<Listing> {
        self.listing_at(None)
    }

    /// Begin a sorted latest-wins listing at the first key at or above `key`.
    /// Skips earlier index subtrees and reads no blob payloads.
    pub fn listing_from(&mut self, key: u64) -> io::Result<Listing> {
        self.listing_at(Some(key))
    }

    fn listing_at(&mut self, key: Option<u64>) -> io::Result<Listing> {
        let mut states = Vec::new();
        let mut segment = self.snapshot.clone();
        loop {
            if states.len() >= self.limits.max_segments {
                return Err(bad("segment limit"));
            }
            let mut scan = Scan::new(segment.clone());
            scan.min_key = key;
            states.push(scan);
            match self.previous(&segment)? {
                Some(prev) => segment = prev,
                None => break,
            }
        }
        let mut listing = Listing {
            states,
            heap: BinaryHeap::new(),
            ready: Vec::new(),
            end: self.snapshot.end,
            root_hash: self.snapshot.root.hash,
            failed: false,
        };
        for i in 0..listing.states.len() {
            let entry = listing.states[i].next(self)?;
            if let Some(e) = &entry {
                listing.heap.push(Reverse((e.key, i)));
            }
            listing.ready.push(entry);
        }
        Ok(listing)
    }
    /// Query cached width/height declarations without fetching blob payloads.
    /// New branch pages prune disjoint subtrees; older branch pages are scanned.
    /// Results are newest-segment first, then ascending key within each segment.
    /// A point lookup verifies each candidate against later replacements, so a
    /// nonmatching replacement cannot expose an older matching declaration.
    pub fn dimension_listing(
        &mut self,
        width: RangeInclusive<u32>,
        height: RangeInclusive<u32>,
    ) -> io::Result<DimensionListing> {
        let bounds = (
            (*width.start(), *width.end()),
            (*height.start(), *height.end()),
        );
        if bounds.0.0 > bounds.0.1 || bounds.1.0 > bounds.1.1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty dimension range",
            ));
        }
        self.filtered_listing(bounds.0, bounds.1, None, None)
    }
    /// Query entries declaring exactly this CICP value, or no CICP when `None`.
    /// Kind 5 branches prune disjoint code-point ranges; older pages are scanned.
    /// Declarations are cached index hints, not the contained image's authority.
    pub fn cicp_listing(&mut self, cicp: Option<imij::Cicp>) -> io::Result<CicpListing> {
        Ok(CicpListing(self.filtered_listing(
            (0, u32::MAX),
            (0, u32::MAX),
            Some(CicpBounds::key(cicp)),
            None,
        )?))
    }
    /// Query entries with an exact indexed filename. Kind 6 branch pages prune
    /// disjoint 16-byte UTF-8 prefix ranges; older pages are scanned. The leaf
    /// always checks the complete filename, including prefix collisions.
    pub fn filename_listing(&mut self, filename: &str) -> io::Result<FilenameListing> {
        if filename.is_empty() || filename.len() > 4096 || filename.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid indexed filename",
            ));
        }
        Ok(FilenameListing(self.filtered_listing(
            (0, u32::MAX),
            (0, u32::MAX),
            None,
            Some(filename.to_owned()),
        )?))
    }
    fn filtered_listing(
        &mut self,
        width: (u32, u32),
        height: (u32, u32),
        cicp: Option<u32>,
        filename: Option<String>,
    ) -> io::Result<DimensionListing> {
        let mut states = Vec::new();
        let mut segment = self.snapshot.clone();
        loop {
            if states.len() >= self.limits.max_segments {
                return Err(bad("segment limit"));
            }
            states.push(DimensionScan {
                segment: segment.clone(),
                stack: vec![(segment.root, segment.level)],
                leaf: Vec::new().into_iter(),
                last: None,
                prior_end: 0,
            });
            match self.previous(&segment)? {
                Some(prev) => segment = prev,
                None => break,
            }
        }
        Ok(DimensionListing {
            states,
            current: 0,
            width,
            height,
            cicp,
            filename,
            end: self.snapshot.end,
            root_hash: self.snapshot.root.hash,
            failed: false,
        })
    }
    /// Copy an entire entry while independently checking its indexed digest.
    /// A failed checksum may occur after bytes reached the sink; use a temporary
    /// output plus atomic rename when unverified bytes must not be exposed.
    pub fn copy_to<W: Write>(&mut self, entry: &Entry, out: &mut W) -> io::Result<()> {
        if self.audit_receipt.is_some() {
            return Err(crate::policy::denied());
        }
        self.check_public_entry(entry)?;
        let mut buffer = [0; 64 * 1024];
        let mut hash = Hasher::new(entry.checksum.algorithm());
        let mut pos = 0;
        while pos < entry.length {
            let n = (entry.length - pos).min(buffer.len() as u64) as usize;
            self.source
                .read_exact_at(add(entry.offset, pos)?, &mut buffer[..n])
                .map_err(source_error)?;
            hash.update(&buffer[..n]);
            out.write_all(&buffer[..n])?;
            pos += n as u64;
        }
        if hash.finish() != entry.checksum {
            return Err(bad("entry checksum mismatch"));
        }
        Ok(())
    }
    /// Read one entry into a single allocation, returning bytes only after its
    /// indexed digest verifies. The caller's limit is checked before reserving
    /// memory. Use [`Self::copy_to`] for larger entries or streaming sinks.
    pub fn read_entry_bounded(&mut self, entry: &Entry, max_bytes: usize) -> io::Result<Vec<u8>> {
        self.check_public_entry(entry)?;
        let len = usize::try_from(entry.length)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "entry exceeds usize"))?;
        if len > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "entry exceeds caller byte limit",
            ));
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(io::Error::other)?;
        bytes.resize(len, 0);
        let mut hash = Hasher::new(entry.checksum.algorithm());
        // The caller has already accepted the complete entry in memory. One
        // positional read lets an HTTP ReadAt source use one range request.
        // Keep the digest check before exposing any of the returned bytes.
        if !bytes.is_empty() {
            self.source
                .read_exact_at(entry.offset, &mut bytes)
                .map_err(source_error)?;
        }
        hash.update(&bytes);
        if hash.finish() != entry.checksum {
            return Err(bad("entry checksum mismatch"));
        }
        Ok(bytes)
    }
    fn check_entry(&mut self, e: &Entry) -> io::Result<()> {
        if add(e.offset, e.length)? > self.snapshot.before() {
            return Err(bad("entry outside snapshot"));
        }
        // A descriptor from another reader can address an unrelated payload at
        // the same offset. Recheck it against this reader's authoritative index.
        if !e
            .owner
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, &self.owner))
            && self.get(e.key)?.as_ref() != Some(e)
        {
            return Err(bad("entry belongs to another archive snapshot"));
        }
        Ok(())
    }

    fn check_public_entry(&mut self, entry: &Entry) -> io::Result<()> {
        self.check_entry(entry)?;
        if entry.access_policy().is_some() {
            return Err(crate::policy::denied());
        }
        Ok(())
    }

    /// Bounded view of a standalone entry for IMI RegionReader or other ReadAt consumers.
    /// Partial reads do not verify the whole-blob digest; IMI's own piece checksums apply.
    pub fn entry_source<'a>(&'a mut self, entry: &Entry) -> io::Result<EntrySource<'a, S>> {
        self.check_public_entry(entry)?;
        Ok(self.bounded_entry_source(entry))
    }

    fn bounded_entry_source(&mut self, entry: &Entry) -> EntrySource<'_, S> {
        EntrySource {
            source: &mut self.source,
            offset: entry.offset,
            length: entry.length,
        }
    }

    /// Open the scope's ordinary nested archive after both memory attestations,
    /// interactive approval and durable audit acknowledgement.
    pub fn open_governed_scope(
        &mut self,
        key: u64,
        run: crate::EvaluationRun<'_>,
        policy: &mut impl crate::HeldOutAccess,
    ) -> io::Result<Reader<EntrySource<'_, S>>> {
        use crate::{AccessDecision, ScopeAccess};

        let entry = self
            .get(key)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "governed scope key"))?;
        let (profile, split_policy) = entry
            .access_policy()
            .ok_or_else(|| bad("entry has no governed scope declaration"))?;
        if profile != crate::policy::PLAINTEXT {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "access profile"));
        }
        if [run.actor, run.run_id, run.frozen_revision, run.reason]
            .iter()
            .any(|field| field.trim().is_empty())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "blank evaluation run field",
            ));
        }
        let access = ScopeAccess {
            run,
            entry_key: key,
            entry_digest: entry.checksum(),
            archive_generation: self.snapshot.generation(),
            split_policy,
        };
        let approval = (|| {
            policy.attest_that_encoded_images_and_decoded_pixels_stay_only_in_memory(&access)?;
            policy.attest_that_human_ratings_and_other_associated_scores_stay_only_in_memory(
                &access,
            )?;
            let approval =
                policy.obtain_interactive_user_approval_for_this_held_out_run(&access)?;
            if approval.trim().is_empty() {
                return Err(crate::policy::denied());
            }
            Ok(approval)
        })();
        let approval_id = match approval {
            Ok(id) => id,
            Err(error) => {
                policy.append_scope_access_decision_to_the_durable_audit_log(
                    &access,
                    AccessDecision::Denied,
                )?;
                return Err(error);
            }
        };
        let receipt = policy.append_scope_access_decision_to_the_durable_audit_log(
            &access,
            AccessDecision::Granted {
                approval_id: &approval_id,
            },
        )?;
        if receipt.trim().is_empty() {
            return Err(crate::policy::denied());
        }
        let mut nested = Reader::open(self.bounded_entry_source(&entry))?;
        nested.audit_receipt = Some(receipt);
        Ok(nested)
    }

    /// Durable authorization acknowledgement for an opened governed scope.
    pub fn audit_receipt(&self) -> Option<&str> {
        self.audit_receipt.as_deref()
    }
}
#[derive(Debug)]
enum Page {
    Leaf(Vec<Entry>),
    Branch(Vec<Node>),
}

// Validation returns the parsed contents so callers cannot accidentally use an
// unchecked page or repeat metadata parsing and allocation after admission.
fn validate_page(
    bytes: &[u8],
    node: Node,
    level: u8,
    before: u64,
    algo: HashAlgorithm,
) -> io::Result<Page> {
    if node.len < 4
        || node.len > MAX_PAGE as u64
        || add(node.offset, node.len)? > before
        || bytes.len() as u64 != node.len
        || !node.hash.matches(algo.digest(bytes))
    {
        return Err(bad("index page bounds/checksum"));
    }
    page_header(bytes, level)?;
    let (actual, bounds, cicp_bounds, filename_bounds, page) = if level == 0 {
        let entries = entries(bytes, node.offset, algo)?;
        (
            entries.last().map(|e| e.key),
            Bounds::from_entries(&entries),
            CicpBounds::from_entries(&entries),
            FilenameBounds::from_entries(&entries),
            Page::Leaf(entries),
        )
    } else {
        let children = children(bytes, level, node.offset, algo)?;
        (
            children.last().and_then(|n| n.max),
            Bounds::from_nodes(&children),
            CicpBounds::from_nodes(&children),
            FilenameBounds::from_nodes(&children),
            Page::Branch(children),
        )
    };
    if node.max.is_some() && node.max != actual {
        return Err(bad("index child maximum differs from contents"));
    }
    if node.bounds.is_some() && node.bounds != bounds {
        return Err(bad("index child dimension bounds differ from contents"));
    }
    if node.cicp_bounds.is_some() && node.cicp_bounds != cicp_bounds {
        return Err(bad("index child CICP bounds differ from contents"));
    }
    if node.filename_bounds.is_some() && node.filename_bounds != filename_bounds {
        return Err(bad("index child filename bounds differ from contents"));
    }
    Ok(page)
}
fn read_page<S: ReadAt>(
    source: &mut S,
    snapshot: &Snapshot,
    node: Node,
    level: u8,
) -> io::Result<(Vec<u8>, Page)> {
    if node.len < 4 || node.len > MAX_PAGE as u64 || add(node.offset, node.len)? > snapshot.before()
    {
        return Err(bad("page bounds"));
    }
    let mut bytes = vec![0; node.len as usize];
    source
        .read_exact_at(node.offset, &mut bytes)
        .map_err(source_error)?;
    let page = validate_page(&bytes, node, level, snapshot.before(), snapshot.algo)?;
    Ok((bytes, page))
}

pub(crate) struct Scan {
    segment: Snapshot,
    stack: Vec<(Node, u8)>,
    leaf: std::vec::IntoIter<Entry>,
    prefetched: std::vec::IntoIter<Page>,
    seen: u64,
    expected: u64,
    min_key: Option<u64>,
    pruned: bool,
    last: Option<u64>,
    prior_end: u64,
}
impl Scan {
    pub(crate) fn new(segment: Snapshot) -> Self {
        Self {
            stack: vec![(segment.root, segment.level)],
            expected: segment.entries,
            min_key: None,
            pruned: false,
            segment,
            leaf: Vec::new().into_iter(),
            prefetched: Vec::new().into_iter(),
            seen: 0,
            last: None,
            prior_end: 0,
        }
    }

    pub(crate) fn next<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        loop {
            if let Some(e) = self.leaf.next() {
                if self.last.is_some_and(|k| k >= e.key) {
                    return Err(bad("segment key order"));
                }
                if e.offset < self.prior_end {
                    return Err(bad("segment blob order/overlap"));
                }
                self.prior_end = add(e.offset, e.length)?;
                self.last = Some(e.key);
                self.seen = add(self.seen, 1)?;
                if self.seen > self.expected {
                    return Err(bad("segment entry count"));
                }
                if self.min_key.is_none_or(|key| e.key >= key) {
                    return Ok(Some(e.bind(&reader.owner)));
                }
                continue;
            }
            if let Some(page) = self.prefetched.next() {
                let Page::Leaf(entries) = page else {
                    unreachable!("level zero is a leaf");
                };
                self.leaf = entries.into_iter();
                continue;
            }
            let Some((node, level)) = self.stack.pop() else {
                if !self.pruned && self.seen != self.expected {
                    return Err(bad("segment entry count"));
                }
                return Ok(None);
            };
            if self
                .min_key
                .is_some_and(|key| node.max.is_some_and(|max| max < key))
            {
                self.pruned = true;
                continue;
            }
            if level == 0 {
                let mut nodes = vec![node];
                let prefetch = if self.min_key.is_some() { 2 } else { 16 };
                while nodes.len() < prefetch
                    && self.stack.last().is_some_and(|(_, level)| *level == 0)
                {
                    nodes.push(self.stack.pop().unwrap().0);
                }
                let mut budget = BatchBudget {
                    pages: nodes.len(),
                    bytes: nodes.len() as u64 * MAX_PAGE as u64,
                    range_bytes: MAX_PAGE as u64,
                    gap: 0,
                };
                self.prefetched = reader
                    .pages(&self.segment, &nodes, 0, &mut budget)?
                    .into_iter();
            } else {
                let Page::Branch(children) = reader.page(&self.segment, node, level)? else {
                    unreachable!("nonzero level is a branch");
                };
                for child in children.into_iter().rev() {
                    self.stack.push((child, level - 1));
                }
            }
        }
    }
}
struct DimensionScan {
    segment: Snapshot,
    stack: Vec<(Node, u8)>,
    leaf: std::vec::IntoIter<Entry>,
    last: Option<u64>,
    prior_end: u64,
}
impl DimensionScan {
    fn next<S: ReadAt>(
        &mut self,
        reader: &mut Reader<S>,
        width: (u32, u32),
        height: (u32, u32),
        cicp: Option<u32>,
        filename: Option<&str>,
    ) -> io::Result<Option<Entry>> {
        loop {
            if let Some(entry) = self.leaf.next() {
                if self.last.is_some_and(|key| key >= entry.key) {
                    return Err(bad("segment key order"));
                }
                if entry.offset < self.prior_end {
                    return Err(bad("segment blob order/overlap"));
                }
                self.prior_end = add(entry.offset, entry.length)?;
                self.last = Some(entry.key);
                if width.0 <= entry.summary.width
                    && entry.summary.width <= width.1
                    && height.0 <= entry.summary.height
                    && entry.summary.height <= height.1
                    && cicp.is_none_or(|key| key == CicpBounds::key(entry.summary.cicp))
                    && filename.is_none_or(|name| entry.metadata.filename.as_deref() == Some(name))
                {
                    return Ok(Some(entry.bind(&reader.owner)));
                }
                continue;
            }
            let Some((node, level)) = self.stack.pop() else {
                return Ok(None);
            };
            if node
                .bounds
                .is_some_and(|bounds| !bounds.intersects(width, height))
            {
                continue;
            }
            if cicp.is_some_and(|key| node.cicp_bounds.is_some_and(|bounds| !bounds.contains(key)))
            {
                continue;
            }
            if filename.is_some_and(|name| {
                node.filename_bounds
                    .is_some_and(|bounds| !bounds.contains(FilenameBounds::key(Some(name))))
            }) {
                continue;
            }
            match reader.page(&self.segment, node, level)? {
                Page::Leaf(entries) => self.leaf = entries.into_iter(),
                Page::Branch(children) => {
                    for child in children.into_iter().rev() {
                        self.stack.push((child, level - 1));
                    }
                }
            }
        }
    }
}
/// Incremental dimension query over cached index declarations.
/// Results are not globally key-sorted across append segments.
pub struct DimensionListing {
    states: Vec<DimensionScan>,
    current: usize,
    width: (u32, u32),
    height: (u32, u32),
    cicp: Option<u32>,
    filename: Option<String>,
    end: u64,
    root_hash: PageHash,
    failed: bool,
}
impl DimensionListing {
    pub fn next<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        if self.failed {
            return Err(bad("filtered listing failed earlier"));
        }
        let result = self.next_inner(reader);
        self.failed = result.is_err();
        result
    }
    fn next_inner<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        if reader.snapshot.end != self.end || reader.snapshot.root.hash != self.root_hash {
            return Err(bad("dimension listing belongs to another snapshot"));
        }
        while self.current < self.states.len() {
            let Some(candidate) = self.states[self.current].next(
                reader,
                self.width,
                self.height,
                self.cicp,
                self.filename.as_deref(),
            )?
            else {
                self.current += 1;
                continue;
            };
            if self.current == 0
                || reader
                    .get(candidate.key)?
                    .is_some_and(|latest| latest.offset == candidate.offset)
            {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }
}
/// Incremental exact-CICP query over cached index declarations.
/// Results are not globally key-sorted across append segments.
pub struct CicpListing(DimensionListing);
impl CicpListing {
    pub fn next<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        self.0.next(reader)
    }
}
/// Incremental exact-filename query over cached index metadata.
/// Results are not globally key-sorted across append segments.
pub struct FilenameListing(DimensionListing);
impl FilenameListing {
    pub fn next<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        self.0.next(reader)
    }
}
/// Incremental sorted listing. Repeated keys emit the newest segment's entry once.
pub struct Listing {
    states: Vec<Scan>,
    heap: BinaryHeap<Reverse<(u64, usize)>>,
    ready: Vec<Option<Entry>>,
    end: u64,
    root_hash: PageHash,
    failed: bool,
}
impl Listing {
    /// Scan cached summaries/metadata after latest-key resolution, without fetching blobs.
    /// This is a bounded sequential index scan. Use `Reader::filename_listing`
    /// for exact names; typed-attribute predicates still scan.
    pub fn next_matching<S: ReadAt>(
        &mut self,
        reader: &mut Reader<S>,
        mut predicate: impl FnMut(&Entry) -> bool,
    ) -> io::Result<Option<Entry>> {
        while let Some(entry) = self.next(reader)? {
            if predicate(&entry) {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }
    pub fn next<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        if self.failed {
            return Err(bad("listing failed earlier"));
        }
        let result = self.next_inner(reader);
        self.failed = result.is_err();
        result
    }
    fn next_inner<S: ReadAt>(&mut self, reader: &mut Reader<S>) -> io::Result<Option<Entry>> {
        if reader.snapshot.end != self.end || reader.snapshot.root.hash != self.root_hash {
            return Err(bad("listing belongs to another snapshot"));
        }
        let Some(Reverse((key, index))) = self.heap.peek().copied() else {
            return Ok(None);
        };
        let result = self.ready[index].take().unwrap();
        while self.heap.peek().is_some_and(|Reverse((k, _))| *k == key) {
            let Reverse((_, i)) = self.heap.pop().unwrap();
            let next = self.states[i].next(reader)?;
            if let Some(e) = &next {
                self.heap.push(Reverse((e.key, i)));
            }
            self.ready[i] = next;
        }
        Ok(Some(result))
    }
}

/// Entry-relative offsets, including batched ROI requests, translated without copying pixels.
pub struct EntrySource<'a, S> {
    source: &'a mut S,
    offset: u64,
    length: u64,
}
impl<S: ReadAt> ReadAt for EntrySource<'_, S> {
    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.length)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(imij::Error::Read);
        }
        self.source.read_exact_at(
            self.offset
                .checked_add(offset)
                .ok_or(imij::Error::TooLarge)?,
            out,
        )
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
        for (offset, out) in requests.iter() {
            if offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > self.length)
                || self.offset.checked_add(*offset).is_none()
            {
                return Err(imij::Error::Read);
            }
        }
        for (offset, _) in requests.iter_mut() {
            *offset += self.offset;
        }
        let result = self.source.read_many_at(requests);
        for (offset, _) in requests.iter_mut() {
            *offset -= self.offset;
        }
        result
    }
}

#[cfg(test)]
mod structural_tests {
    use super::*;
    use imij::planes::SliceReader;

    // Deliberately invalid index structures, with valid hashes at every layer.
    fn archive(wrong_max: bool, overlap: bool, branch_kind: u8) -> Vec<u8> {
        assert!([1, 4, 5].contains(&branch_kind));
        let mut bytes = b"ab".to_vec();
        let mut nodes = Vec::new();
        for i in 0..2u64 {
            let offset = if overlap { 0 } else { i };
            let mut metadata = crate::EntryMetadata::default();
            metadata.filename = Some("legacy.imij".into());
            let data = leaf(&[Entry {
                owner: None,
                key: i + 1,
                offset,
                length: 1,
                head: 0,
                checksum: HashAlgorithm::Xxh3_64
                    .digest(&bytes[offset as usize..offset as usize + 1]),
                summary: Summary::default(),
                metadata,
            }]);
            nodes.push(Node {
                max: Some(if wrong_max && i == 0 { 0 } else { i + 1 }),
                offset: bytes.len() as u64,
                len: data.len() as u64,
                hash: HashAlgorithm::Xxh3_64.digest(&data).into(),
                bounds: None,
                cicp_bounds: None,
                filename_bounds: None,
            });
            bytes.extend_from_slice(&data);
        }
        let mut branch = vec![branch_kind, 1, 2, 0];
        for node in nodes {
            node.write(&mut branch);
            if branch_kind >= 4 {
                Bounds {
                    width_min: 0,
                    width_max: 0,
                    height_min: 0,
                    height_max: 0,
                }
                .write(&mut branch);
            }
            if branch_kind >= 5 {
                CicpBounds { min: 0, max: 0 }.write(&mut branch);
            }
        }
        let root = Node {
            max: None,
            offset: bytes.len() as u64,
            len: branch.len() as u64,
            hash: HashAlgorithm::Xxh3_64.digest(&branch).into(),
            bounds: None,
            cicp_bounds: None,
            filename_bounds: None,
        };
        bytes.extend_from_slice(&branch);
        let footer = Snapshot {
            end: bytes.len() as u64 + FOOTER as u64,
            previous: NONE,
            root,
            entries: 2,
            level: 1,
            generation: 1,
            algo: HashAlgorithm::Xxh3_64,
            footer_len: FOOTER_V1,
        };
        bytes.extend_from_slice(&footer.encode().unwrap());
        bytes
    }
    #[test]
    fn full_listing_rejects_wrong_child_max_and_cross_leaf_overlap() {
        let control = archive(false, false, 1);
        let mut r = Reader::open(SliceReader::new(&control)).unwrap();
        let mut scan = r.listing().unwrap();
        assert_eq!(scan.next(&mut r).unwrap().unwrap().key(), 1);
        assert_eq!(scan.next(&mut r).unwrap().unwrap().key(), 2);
        assert!(scan.next(&mut r).unwrap().is_none());

        let wrong = archive(true, false, 1);
        let mut r = Reader::open(SliceReader::new(&wrong)).unwrap();
        assert_eq!(
            r.listing().err().unwrap().to_string(),
            "index child maximum differs from contents"
        );
        let wrong = archive(false, true, 1);
        let mut r = Reader::open(SliceReader::new(&wrong)).unwrap();
        let mut scan = r.listing().unwrap();
        assert_eq!(
            scan.next(&mut r).unwrap_err().to_string(),
            "segment blob order/overlap"
        );
        assert!(scan.next(&mut r).is_err());

        for branch_kind in [1, 4, 5] {
            let legacy = archive(false, false, branch_kind);
            let mut r = Reader::open(SliceReader::new(&legacy)).unwrap();
            let mut query = r.dimension_listing(0..=0, 0..=0).unwrap();
            assert_eq!(query.next(&mut r).unwrap().unwrap().key(), 1);
            assert_eq!(query.next(&mut r).unwrap().unwrap().key(), 2);
            assert!(query.next(&mut r).unwrap().is_none());
            let mut query = r.cicp_listing(None).unwrap();
            assert_eq!(query.next(&mut r).unwrap().unwrap().key(), 1);
            assert_eq!(query.next(&mut r).unwrap().unwrap().key(), 2);
            assert!(query.next(&mut r).unwrap().is_none());
            let mut query = r.filename_listing("legacy.imij").unwrap();
            assert_eq!(query.next(&mut r).unwrap().unwrap().key(), 1);
            assert_eq!(query.next(&mut r).unwrap().unwrap().key(), 2);
            assert!(query.next(&mut r).unwrap().is_none());
        }
    }
    #[test]
    fn fetched_leaf_rejects_forged_cicp_bounds() {
        let summary = Summary {
            cicp: Some(imij::Cicp::SRGB),
            ..Summary::default()
        };
        let bytes = leaf(&[Entry {
            owner: None,
            key: 1,
            offset: 0,
            length: 1,
            head: 0,
            checksum: HashAlgorithm::Xxh3_64.digest(b"x"),
            summary,
            metadata: crate::EntryMetadata::default(),
        }]);
        let node = Node {
            max: Some(1),
            offset: 1,
            len: bytes.len() as u64,
            hash: HashAlgorithm::Xxh3_64.digest(&bytes).into(),
            bounds: None,
            cicp_bounds: Some(CicpBounds { min: 0, max: 0 }),
            filename_bounds: None,
        };
        assert_eq!(
            validate_page(
                &bytes,
                node,
                0,
                node.offset + node.len,
                HashAlgorithm::Xxh3_64
            )
            .unwrap_err()
            .to_string(),
            "index child CICP bounds differ from contents"
        );
    }
    #[test]
    fn fetched_leaf_rejects_forged_filename_bounds() {
        let mut metadata = crate::EntryMetadata::default();
        metadata.filename = Some("actual.imij".into());
        let bytes = leaf(&[Entry {
            owner: None,
            key: 1,
            offset: 0,
            length: 1,
            head: 0,
            checksum: HashAlgorithm::Xxh3_64.digest(b"x"),
            summary: Summary::default(),
            metadata,
        }]);
        let node = Node {
            max: Some(1),
            offset: 1,
            len: bytes.len() as u64,
            hash: HashAlgorithm::Xxh3_64.digest(&bytes).into(),
            bounds: None,
            cicp_bounds: None,
            filename_bounds: Some(FilenameBounds { min: 0, max: 0 }),
        };
        assert_eq!(
            validate_page(
                &bytes,
                node,
                0,
                node.offset + node.len,
                HashAlgorithm::Xxh3_64
            )
            .unwrap_err()
            .to_string(),
            "index child filename bounds differ from contents"
        );
    }
}
