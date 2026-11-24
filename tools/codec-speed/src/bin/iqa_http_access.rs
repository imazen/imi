//! Instrumented HTTP 206 access to one complete imij-set entry.
#![forbid(unsafe_code)]

use imij::planes::ReadAt;
use imij_set::Reader;
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    fs,
    io::{self, Read},
    rc::Rc,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[path = "iqa_http_access/cache.rs"]
mod cache;
// One bounded window per client. Archive offsets may be many gigabytes wide;
// only nearby bytes are retained. Large reads and uncached batches keep their
// ordinary positional behavior.
const CACHE_WINDOW: usize = 256 * 1024;
const SMALL_READ: usize = 64 * 1024;

struct CachedReadAt<S> {
    inner: S,
    len: u64,
    window: Option<(u64, Vec<u8>)>,
}

impl<S: ReadAt> CachedReadAt<S> {
    fn new(mut inner: S) -> std::result::Result<Self, imij::Error> {
        let len = inner.len()?;
        Ok(Self {
            inner,
            len,
            window: None,
        })
    }

    fn cached_prefix(&self, offset: u64, out: &mut [u8]) -> usize {
        let Some((start, bytes)) = &self.window else {
            return 0;
        };
        let Some(at) = offset
            .checked_sub(*start)
            .and_then(|n| usize::try_from(n).ok())
        else {
            return 0;
        };
        let n = out.len().min(bytes.len().saturating_sub(at));
        if n != 0 {
            out[..n].copy_from_slice(&bytes[at..at + n]);
        }
        n
    }
}

impl<S: ReadAt> ReadAt for CachedReadAt<S> {
    fn len(&mut self) -> std::result::Result<u64, imij::Error> {
        Ok(self.len)
    }

    fn read_exact_at(
        &mut self,
        offset: u64,
        out: &mut [u8],
    ) -> std::result::Result<(), imij::Error> {
        offset
            .checked_add(out.len() as u64)
            .filter(|&end| end <= self.len)
            .ok_or(imij::Error::Read)?;
        if out.is_empty() {
            return Ok(());
        }
        let cached = self.cached_prefix(offset, out);
        if cached == out.len() {
            return Ok(());
        }
        if cached != 0 {
            return self
                .inner
                .read_exact_at(offset + cached as u64, &mut out[cached..]);
        }
        if out.len() < SMALL_READ {
            let n = (self.len - offset).min(CACHE_WINDOW as u64) as usize;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(n)
                .map_err(|_| imij::Error::TooLarge)?;
            bytes.resize(n, 0);
            self.inner.read_exact_at(offset, &mut bytes)?;
            out.copy_from_slice(&bytes[..out.len()]);
            self.window = Some((offset, bytes));
            return Ok(());
        }
        self.inner.read_exact_at(offset, out)
    }

    fn read_many_at(
        &mut self,
        requests: &mut [(u64, &mut [u8])],
    ) -> std::result::Result<(), imij::Error> {
        let mut missing = Vec::new();
        missing
            .try_reserve_exact(requests.len())
            .map_err(|_| imij::Error::TooLarge)?;
        for (offset, out) in requests {
            if offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > self.len)
            {
                return Err(imij::Error::Read);
            }
            if self.cached_prefix(*offset, out) != out.len() {
                missing.push((*offset, &mut **out));
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            self.inner.read_many_at(&mut missing)
        }
    }
}

// Preserve the original read-ahead policy as an explicit control. Exact caching
// is the independent-image default after matched range and real R2 checks.
// Indexed sequences retain read-ahead; the environment selects either control.
enum PolicyCache<S> {
    Prefetch(CachedReadAt<S>),
    Exact(cache::ExactReadCache<S>),
}
impl<S: ReadAt> PolicyCache<S> {
    fn new(inner: S) -> std::result::Result<Self, imij::Error> {
        Self::with_default(inner, "prefetch")
    }
    fn with_default(inner: S, default: &str) -> std::result::Result<Self, imij::Error> {
        match std::env::var("IMI_HTTP_CACHE")
            .as_deref()
            .unwrap_or(default)
        {
            "prefetch" => Ok(Self::Prefetch(CachedReadAt::new(inner)?)),
            "exact" => Ok(Self::Exact(cache::ExactReadCache::new(inner)?)),
            _ => Err(imij::Error::InvalidInput("unknown HTTP cache policy")),
        }
    }
}
impl<S: ReadAt> ReadAt for PolicyCache<S> {
    fn len(&mut self) -> std::result::Result<u64, imij::Error> {
        match self {
            Self::Prefetch(s) => s.len(),
            Self::Exact(s) => s.len(),
        }
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> std::result::Result<(), imij::Error> {
        match self {
            Self::Prefetch(s) => s.read_exact_at(at, out),
            Self::Exact(s) => s.read_exact_at(at, out),
        }
    }
    fn read_many_at(
        &mut self,
        reqs: &mut [(u64, &mut [u8])],
    ) -> std::result::Result<(), imij::Error> {
        match self {
            Self::Prefetch(s) => s.read_many_at(reqs),
            Self::Exact(s) => s.read_many_at(reqs),
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

struct RangeSource {
    agent: ureq::Agent,
    url: String,
    length: Option<u64>,
    etag: Option<String>,
    requests: Rc<Cell<u64>>,
    body_bytes: Rc<Cell<u64>>,
    ranges: Rc<RefCell<Vec<(u64, usize)>>>,
}

impl RangeSource {
    fn open(url: String) -> Result<Self> {
        // Private signed object URLs stay in mode-600 files, out of argv/logs.
        let url = if let Some(path) = url.strip_prefix('@') {
            fs::read_to_string(path)?.trim().to_owned()
        } else {
            url
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .max_redirects(0)
            .build()
            .into();
        let mut source = Self {
            agent,
            url,
            length: None,
            etag: None,
            requests: Rc::new(Cell::new(0)),
            body_bytes: Rc::new(Cell::new(0)),
            ranges: Rc::new(RefCell::new(Vec::new())),
        };
        source.fetch(0, &mut [0])?;
        Ok(source)
    }

    fn fetch(&mut self, offset: u64, out: &mut [u8]) -> Result<()> {
        if out.is_empty() {
            return Ok(());
        }
        let end = offset
            .checked_add(out.len() as u64 - 1)
            .ok_or_else(|| invalid("range overflow"))?;
        if self.length.is_some_and(|length| end >= length) {
            return Err(invalid("range past archive end").into());
        }
        let range = format!("bytes={offset}-{end}");
        let mut request = self
            .agent
            .get(&self.url)
            .header("Range", &range)
            .header("Accept-Encoding", "identity");
        if let Some(tag) = &self.etag {
            request = request.header("If-Match", tag);
        }
        let mut response = request.call()?;
        if response.status().as_u16() != 206 {
            return Err(invalid("HTTP source did not return 206").into());
        }
        let headers = response.headers();
        let content_range = headers
            .get("Content-Range")
            .ok_or_else(|| invalid("missing Content-Range"))?
            .to_str()?
            .to_owned();
        let content_length: usize = headers
            .get("Content-Length")
            .ok_or_else(|| invalid("missing Content-Length"))?
            .to_str()?
            .parse()?;
        if content_length != out.len() || headers.contains_key("Content-Encoding") {
            return Err(invalid("HTTP range length or encoding changed").into());
        }
        let prefix = format!("bytes {offset}-{end}/");
        let total: u64 = content_range
            .strip_prefix(&prefix)
            .ok_or_else(|| invalid("Content-Range differs from request"))?
            .parse()?;
        if total <= end || self.length.is_some_and(|length| length != total) {
            return Err(invalid("HTTP representation length changed").into());
        }
        let tag = headers
            .get("ETag")
            .ok_or_else(|| invalid("missing ETag"))?
            .to_str()?
            .to_owned();
        if !tag.starts_with('"')
            || !tag.ends_with('"')
            || tag.len() < 3
            || self.etag.as_ref().is_some_and(|prior| prior != &tag)
        {
            return Err(invalid("HTTP strong ETag changed").into());
        }
        let mut body = response.body_mut().as_reader();
        body.read_exact(out)?;
        if body.read(&mut [0])? != 0 {
            return Err(invalid("HTTP response has extra body bytes").into());
        }
        self.length = Some(total);
        self.etag = Some(tag);
        self.requests.set(self.requests.get() + 1);
        self.body_bytes
            .set(self.body_bytes.get() + out.len() as u64);
        self.ranges.borrow_mut().push((offset, out.len()));
        Ok(())
    }
}

impl ReadAt for RangeSource {
    fn len(&mut self) -> std::result::Result<u64, imij::Error> {
        self.length.ok_or(imij::Error::Read)
    }

    fn read_exact_at(
        &mut self,
        offset: u64,
        out: &mut [u8],
    ) -> std::result::Result<(), imij::Error> {
        self.fetch(offset, out).map_err(|_| imij::Error::Read)
    }

    fn read_many_at(
        &mut self,
        requests: &mut [(u64, &mut [u8])],
    ) -> std::result::Result<(), imij::Error> {
        // Index leaves are separated by small blob payloads. Fetch a bounded
        // enclosing range for nearby leaves so each batch costs one HTTP RTT.
        const MAX_SPAN: u64 = 1024 * 1024;
        const MAX_GAP: u64 = 4096;
        let mut order: Vec<usize> = (0..requests.len())
            .filter(|&index| !requests[index].1.is_empty())
            .collect();
        order.sort_unstable_by_key(|&index| requests[index].0);
        let mut first = 0;
        while first < order.len() {
            let start = requests[order[first]].0;
            let mut end = start
                .checked_add(requests[order[first]].1.len() as u64)
                .ok_or(imij::Error::TooLarge)?;
            let mut last = first + 1;
            while last < order.len() {
                let next = order[last];
                let at = requests[next].0;
                let next_end = at
                    .checked_add(requests[next].1.len() as u64)
                    .ok_or(imij::Error::TooLarge)?;
                let merged_end = end.max(next_end);
                if at.saturating_sub(end) > MAX_GAP || merged_end - start > MAX_SPAN {
                    break;
                }
                end = merged_end;
                last += 1;
            }
            if last == first + 1 {
                let index = order[first];
                self.fetch(requests[index].0, requests[index].1)
                    .map_err(|_| imij::Error::Read)?;
            } else {
                let size = usize::try_from(end - start).map_err(|_| imij::Error::TooLarge)?;
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(size)
                    .map_err(|_| imij::Error::TooLarge)?;
                bytes.resize(size, 0);
                self.fetch(start, &mut bytes)
                    .map_err(|_| imij::Error::Read)?;
                for &index in &order[first..last] {
                    let offset = (requests[index].0 - start) as usize;
                    let len = requests[index].1.len();
                    requests[index]
                        .1
                        .copy_from_slice(&bytes[offset..offset + len]);
                }
            }
            first = last;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SummaryQuery {
    Dimensions { width: u32, height: u32 },
    Cicp(imij::Cicp),
}
impl SummaryQuery {
    fn matches(self, summary: imij_set::Summary) -> bool {
        match self {
            Self::Dimensions { width, height } => {
                summary.width == width && summary.height == height
            }
            Self::Cicp(cicp) => summary.cicp == Some(cicp),
        }
    }
    fn fields(self) -> String {
        match self {
            Self::Dimensions { width, height } => {
                format!("\"query\":\"dimensions\",\"width\":{width},\"height\":{height}")
            }
            Self::Cicp(c) => format!(
                "\"query\":\"cicp\",\"cicp\":[{},{},{},{}]",
                c.color_primaries,
                c.transfer_characteristics,
                c.matrix_coefficients,
                u8::from(c.full_range)
            ),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 9 && args[2] == "--roi" {
        let key: u64 = args[3].parse()?;
        let (x, y, w, h): (u32, u32, u32, u32) = (
            args[4].parse()?,
            args[5].parse()?,
            args[6].parse()?,
            args[7].parse()?,
        );
        let expected = fs::read(&args[8])?;
        let started = Instant::now();
        let source = RangeSource::open(args[1].clone())?;
        let requests = Rc::clone(&source.requests);
        let body_bytes = Rc::clone(&source.body_bytes);
        let ranges = Rc::clone(&source.ranges);
        let mut archive = Reader::open(PolicyCache::with_default(source, "exact")?)?;
        let entry = archive
            .get(key)?
            .ok_or_else(|| invalid("archive key not found"))?;
        let mut region = imij::region::RegionReader::open(archive.entry_source(&entry)?)?;
        let info = region.info();
        let stride = (w as usize)
            .checked_mul(info.channels.count())
            .and_then(|n| n.checked_mul(info.sample.bytes()))
            .ok_or("ROI row overflow")?;
        let size = stride
            .checked_mul(h as usize)
            .filter(|&n| n as u64 <= imij::Limits::DEFAULT.max_bytes)
            .ok_or("ROI output limit")?;
        if expected.len() != size {
            return Err("ROI source extent differs".into());
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(size)?;
        pixels.resize(size, 0);
        region.decode_region(imij::planes::Rect::new(x, y, w, h), &mut pixels, stride)?;
        if pixels != expected {
            return Err(invalid("ROI pixels differ from source crop").into());
        }
        let identity = xxhash_rust::xxh3::xxh3_128(&pixels);
        let ranges = ranges
            .borrow()
            .iter()
            .map(|(offset, len)| format!("[{offset},{len}]"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"key\":{key},\"width\":{w},\"height\":{h},\"decoded_bytes\":{size},\"requests\":{},\"body_bytes\":{},\"ranges\":[{ranges}],\"wall_ms\":{},\"pixel_xxh3_128\":\"{identity:032x}\"}}",
            requests.get(),
            body_bytes.get(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        return Ok(());
    }
    if args.len() == 5 && args[2] == "--image" {
        let key: u64 = args[3].parse()?;
        let started = Instant::now();
        let source = RangeSource::open(args[1].clone())?;
        let requests = Rc::clone(&source.requests);
        let body_bytes = Rc::clone(&source.body_bytes);
        let ranges = Rc::clone(&source.ranges);
        let mut reader = Reader::open(PolicyCache::with_default(source, "exact")?)?;
        let entry = reader
            .get(key)?
            .ok_or_else(|| invalid("archive key not found"))?;
        let encoded = reader.read_entry_bounded(&entry, 64 * 1024 * 1024)?;
        let (info, pixels) = imij::decode(&encoded)?;
        let sha256 = Sha256::digest(&pixels)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if sha256 != args[4] {
            return Err(invalid("independent image pixels differ from source").into());
        }
        let ranges = ranges
            .borrow()
            .iter()
            .map(|(offset, len)| format!("[{offset},{len}]"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"key\":{key},\"width\":{},\"height\":{},\"decoded_bytes\":{},\"requests\":{},\"body_bytes\":{},\"ranges\":[{ranges}],\"wall_ms\":{},\"pixel_sha256\":\"{sha256}\"}}",
            info.width,
            info.height,
            pixels.len(),
            requests.get(),
            body_bytes.get(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        return Ok(());
    }
    if (args.len() == 6 && args[2] == "--indexed")
        || (args.len() == 7 && args[2] == "--indexed-repeat")
    {
        let key: u64 = args[3].parse()?;
        let ordinal: u64 = args[4].parse()?;
        let expected_sha = &args[5];
        let repeats: usize = if args.len() == 7 { args[6].parse()? } else { 1 };
        if !(1..=32).contains(&repeats) {
            return Err("indexed-repeat requires 1..=32 operations".into());
        }
        let started = Instant::now();
        let source = RangeSource::open(args[1].clone())?;
        let requests = Rc::clone(&source.requests);
        let body_bytes = Rc::clone(&source.body_bytes);
        let ranges = Rc::clone(&source.ranges);
        let mut reader = Reader::open(PolicyCache::new(source)?)?;
        let entry = reader
            .get(key)?
            .ok_or_else(|| invalid("archive key not found"))?;
        let mut operations = Vec::new();
        let mut last = None;
        for _ in 0..repeats {
            let before = (requests.get(), body_bytes.get(), Instant::now());
            let (info, pixels) = reader.decode_indexed_item(
                &entry,
                ordinal,
                32,
                64 * 1024 * 1024,
                &imij::Limits::DEFAULT,
                1,
            )?;
            let sha256 = Sha256::digest(&pixels)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if &sha256 != expected_sha {
                return Err(invalid("indexed image pixels differ from source").into());
            }
            operations.push(format!(
                "{{\"requests\":{},\"body_bytes\":{},\"wall_ms\":{}}}",
                requests.get() - before.0,
                body_bytes.get() - before.1,
                before.2.elapsed().as_secs_f64() * 1000.0
            ));
            last = Some((info, pixels.len(), sha256));
        }
        let (info, decoded_bytes, sha256) = last.ok_or("missing indexed operation")?;
        let operations = operations.join(",");
        let ranges = ranges
            .borrow()
            .iter()
            .map(|(offset, len)| format!("[{offset},{len}]"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"key\":{key},\"ordinal\":{ordinal},\"width\":{},\"height\":{},\"decoded_bytes\":{},\"requests\":{},\"body_bytes\":{},\"ranges\":[{ranges}],\"wall_ms\":{},\"pixel_sha256\":\"{sha256}\",\"operations\":[{operations}]}}",
            info.width,
            info.height,
            decoded_bytes,
            requests.get(),
            body_bytes.get(),
            started.elapsed().as_secs_f64() * 1000.0,
        );
        return Ok(());
    }
    let summary_query = if args.len() == 7 && args[2] == "--dimensions" {
        Some((
            SummaryQuery::Dimensions {
                width: args[4].parse()?,
                height: args[5].parse()?,
            },
            args[6].parse::<u64>()?,
        ))
    } else if args.len() == 9 && args[2] == "--cicp" {
        let range: u8 = args[7].parse()?;
        if range > 1 {
            return Err("CICP full-range must be 0 or 1".into());
        }
        Some((
            SummaryQuery::Cicp(imij::Cicp {
                color_primaries: args[4].parse()?,
                transfer_characteristics: args[5].parse()?,
                matrix_coefficients: args[6].parse()?,
                full_range: range == 1,
            }),
            args[8].parse::<u64>()?,
        ))
    } else {
        None
    };
    if let Some((query, expected)) = summary_query {
        let mode = args[3].as_str();
        if mode != "pruned" && mode != "scan" {
            return Err("summary-query mode must be pruned or scan".into());
        }
        let open_start = Instant::now();
        let source = RangeSource::open(args[1].clone())?;
        let requests = Rc::clone(&source.requests);
        let body_bytes = Rc::clone(&source.body_bytes);
        let mut reader = Reader::open(source)?;
        let open_ms = open_start.elapsed().as_secs_f64() * 1000.0;
        let open_requests = requests.get();
        let open_bytes = body_bytes.get();
        let query_start = Instant::now();
        let mut digest = Sha256::new();
        let mut count = 0u64;
        if mode == "pruned" {
            match query {
                SummaryQuery::Dimensions { width, height } => {
                    let mut listing = reader.dimension_listing(width..=width, height..=height)?;
                    while let Some(entry) = listing.next(&mut reader)? {
                        digest.update(entry.key().to_le_bytes());
                        count += 1;
                    }
                }
                SummaryQuery::Cicp(cicp) => {
                    let mut listing = reader.cicp_listing(Some(cicp))?;
                    while let Some(entry) = listing.next(&mut reader)? {
                        digest.update(entry.key().to_le_bytes());
                        count += 1;
                    }
                }
            }
        } else {
            let mut listing = reader.listing()?;
            while let Some(entry) =
                listing.next_matching(&mut reader, |entry| query.matches(entry.summary()))?
            {
                digest.update(entry.key().to_le_bytes());
                count += 1;
            }
        }
        if count != expected {
            return Err(invalid("summary query count differs from expected").into());
        }
        let query_ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let key_sha256 = digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let fields = query.fields();
        println!(
            "{{\"mode\":\"{mode}\",{fields},\"count\":{count},\"key_sha256\":\"{key_sha256}\",\"open_ms\":{open_ms},\"query_ms\":{query_ms},\"open_requests\":{open_requests},\"query_requests\":{},\"open_body_bytes\":{open_bytes},\"query_body_bytes\":{}}}",
            requests.get() - open_requests,
            body_bytes.get() - open_bytes,
        );
        return Ok(());
    }
    if args.len() != 4 {
        return Err("usage: iqa_http_access URL KEY EXPECTED_IMI | URL --image KEY PIXEL_HASH | URL --indexed KEY ORDINAL PIXEL_HASH | URL --indexed-repeat KEY ORDINAL PIXEL_HASH COUNT | URL --dimensions pruned|scan WIDTH HEIGHT EXPECTED_COUNT | URL --cicp pruned|scan PRIMARIES TRANSFER MATRIX FULL_RANGE EXPECTED_COUNT".into());
    }
    let key: u64 = args[2].parse()?;
    let expected = fs::read(&args[3])?;
    let open_start = Instant::now();
    let source = RangeSource::open(args[1].clone())?;
    let requests = Rc::clone(&source.requests);
    let body_bytes = Rc::clone(&source.body_bytes);
    let mut reader = Reader::open(source)?;
    let open_ms = open_start.elapsed().as_secs_f64() * 1000.0;
    let open_requests = requests.get();
    let open_bytes = body_bytes.get();

    let lookup_start = Instant::now();
    let entry = reader
        .get(key)?
        .ok_or_else(|| invalid("archive key not found"))?;
    let lookup_ms = lookup_start.elapsed().as_secs_f64() * 1000.0;
    let lookup_requests = requests.get() - open_requests;
    let lookup_bytes = body_bytes.get() - open_bytes;

    let copy_start = Instant::now();
    let actual = reader.read_entry_bounded(&entry, expected.len())?;
    let copy_ms = copy_start.elapsed().as_secs_f64() * 1000.0;
    if actual != expected {
        return Err(invalid("HTTP entry bytes differ from sealed source").into());
    }
    let digest = Sha256::digest(&actual)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    println!(
        "{{\"key\":{key},\"open_ms\":{open_ms},\"lookup_ms\":{lookup_ms},\"copy_ms\":{copy_ms},\"open_requests\":{open_requests},\"lookup_requests\":{lookup_requests},\"copy_requests\":{},\"open_body_bytes\":{open_bytes},\"lookup_body_bytes\":{lookup_bytes},\"copy_body_bytes\":{},\"entry_bytes\":{},\"sha256\":\"{digest}\"}}",
        requests.get() - open_requests - lookup_requests,
        body_bytes.get() - open_bytes - lookup_bytes,
        actual.len(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SparseSource {
        len: u64,
        reads: Vec<(u64, usize)>,
        batches: usize,
    }

    impl ReadAt for SparseSource {
        fn len(&mut self) -> std::result::Result<u64, imij::Error> {
            Ok(self.len)
        }

        fn read_exact_at(
            &mut self,
            offset: u64,
            out: &mut [u8],
        ) -> std::result::Result<(), imij::Error> {
            if offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > self.len)
            {
                return Err(imij::Error::Read);
            }
            self.reads.push((offset, out.len()));
            for (i, byte) in out.iter_mut().enumerate() {
                *byte = (offset + i as u64) as u8;
            }
            Ok(())
        }

        fn read_many_at(
            &mut self,
            requests: &mut [(u64, &mut [u8])],
        ) -> std::result::Result<(), imij::Error> {
            self.batches += 1;
            for (offset, out) in requests {
                self.read_exact_at(*offset, out)?;
            }
            Ok(())
        }
    }

    #[test]
    fn bounded_cache_reuses_disjoint_spans_in_sparse_ten_gigabyte_archive() {
        let mut source = CachedReadAt::new(SparseSource {
            len: 10 * 1024 * 1024 * 1024 + 100,
            reads: Vec::new(),
            batches: 0,
        })
        .unwrap();
        let base = 5 * 1024 * 1024 * 1024;
        let mut header = [0; 40];
        source.read_exact_at(base, &mut header).unwrap();
        assert_eq!(source.inner.reads, vec![(base, CACHE_WINDOW)]);
        assert_eq!(header[0], base as u8);

        let mut head_a = [0; 20];
        let mut head_b = [0; 20];
        source
            .read_many_at(&mut [(base + 72_251, &mut head_a), (base + 182_563, &mut head_b)])
            .unwrap();
        assert_eq!(source.inner.reads.len(), 1);
        assert_eq!(source.inner.batches, 0);
        assert_eq!(head_a[0], (base + 72_251) as u8);
        assert_eq!(head_b[0], (base + 182_563) as u8);

        let mut encoded = vec![0; 202_129];
        source.read_exact_at(base + 6_647, &mut encoded).unwrap();
        assert_eq!(source.inner.reads.len(), 1);
        assert_eq!(encoded[0], (base + 6_647) as u8);
        assert_eq!(encoded[encoded.len() - 1], (base + 6_647 + 202_128) as u8);
        assert!(source.window.as_ref().unwrap().1.len() <= CACHE_WINDOW);

        let near_end = source.len - 30;
        let mut control = [0; 24];
        source.read_exact_at(near_end, &mut control).unwrap();
        assert_eq!(source.inner.reads[1], (near_end, 30));
        assert_eq!(source.window.as_ref().unwrap().1.len(), 30);
        assert!(source.read_exact_at(source.len - 5, &mut [0; 6]).is_err());
    }

    #[test]
    fn uncached_batches_and_large_reads_keep_positional_behavior() {
        let mut source = CachedReadAt::new(SparseSource {
            len: 12 * 1024 * 1024 * 1024,
            reads: Vec::new(),
            batches: 0,
        })
        .unwrap();
        let mut first = [0; 20];
        let mut second = [0; 20];
        source
            .read_many_at(&mut [(100, &mut first), (10_000_000_000, &mut second)])
            .unwrap();
        assert_eq!(source.inner.batches, 1);
        assert!(source.window.is_none());
        assert_eq!(source.inner.reads, vec![(100, 20), (10_000_000_000, 20)]);

        let mut large = vec![0; CACHE_WINDOW + 1];
        source.read_exact_at(6_000_000_000, &mut large).unwrap();
        assert_eq!(source.inner.reads[2], (6_000_000_000, CACHE_WINDOW + 1));
        assert!(source.window.is_none());
    }
}
