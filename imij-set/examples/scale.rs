//! One process per archive size, suitable for /usr/bin/time -v.
use imij::planes::{ReadAt, SeekReader, SliceReader};
use imij_set::{EntryMetadata, Reader, Summary, Writer};
use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};
struct Meter {
    inner: SeekReader<File>,
    batches: Rc<Cell<u64>>,
    bytes: Rc<Cell<u64>>,
    latency: Duration,
}
impl ReadAt for Meter {
    fn len(&mut self) -> Result<u64, imij::Error> {
        self.inner.len()
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.batches.set(self.batches.get() + 1);
        self.bytes.set(self.bytes.get() + out.len() as u64);
        std::thread::sleep(self.latency);
        self.inner.read_exact_at(at, out)
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
        self.batches.set(self.batches.get() + 1);
        std::thread::sleep(self.latency);
        for (at, out) in requests {
            self.bytes.set(self.bytes.get() + out.len() as u64);
            self.inner.read_exact_at(*at, out)?;
        }
        Ok(())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() == 4 && args[1] == "--read" {
        return read_existing(&args[2], args[3].parse()?);
    }
    if args.len() != 3 {
        return Err("usage: scale OUTPUT COUNT".into());
    }
    let n: u64 = args[2].parse()?;
    if n == 0 {
        return Err("COUNT must be positive".into());
    }
    // One delay per ReadAt batch, including a batched leaf prefetch. This is
    // request latency injection, not a measurement of HTTP transport behavior.
    let latency_ms: u64 =
        std::env::var("IMIJ_SET_INJECT_BATCH_MS").map_or(Ok(0), |value| value.parse())?;
    let append_entries: u64 =
        std::env::var("IMIJ_SET_APPEND_ENTRIES").map_or(Ok(0), |value| value.parse())?;
    let dimension_query = match std::env::var("IMIJ_SET_DIMENSION_QUERY") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Ok(_) => return Err("IMIJ_SET_DIMENSION_QUERY must be 0 or 1".into()),
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => return Err(error.into()),
    };
    let cicp_query = match std::env::var("IMIJ_SET_CICP_QUERY") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Ok(_) => return Err("IMIJ_SET_CICP_QUERY must be 0 or 1".into()),
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => return Err(error.into()),
    };
    let filename_query = match std::env::var("IMIJ_SET_FILENAME_QUERY") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Ok(_) => return Err("IMIJ_SET_FILENAME_QUERY must be 0 or 1".into()),
        Err(std::env::VarError::NotPresent) => false,
        Err(error) => return Err(error.into()),
    };
    if [dimension_query, cicp_query, filename_query]
        .into_iter()
        .filter(|enabled| *enabled)
        .count()
        > 1
    {
        return Err("select one summary query mode".into());
    }
    let total = n
        .checked_add(append_entries)
        .ok_or("entry count overflow")?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    let mut w = Writer::new(BufWriter::new(file));
    let start = Instant::now();
    for k in 0..n {
        let mut value = [0; 16];
        value[..8].copy_from_slice(&k.to_le_bytes());
        value[8..].copy_from_slice(&(!k).to_le_bytes());
        let mut summary = Summary::default();
        if dimension_query {
            summary.width = if k < 256 { 512 } else { 840 };
            summary.height = 940;
        }
        if cicp_query {
            summary.cicp = Some(if k < 256 {
                imij::Cicp::SRGB
            } else {
                imij::Cicp::BT2100_PQ
            });
        }
        if filename_query {
            let mut metadata = EntryMetadata::default();
            metadata.filename = Some(
                if k < 256 {
                    "variants/longname-A.imij"
                } else {
                    "other/image.imij"
                }
                .into(),
            );
            w.insert_with_metadata(k, 16, 0, summary, &metadata, &mut value.as_slice())?;
        } else {
            w.insert(k, 16, 0, summary, &mut value.as_slice())?;
        }
    }
    let (mut sink, snapshot) = w.finish()?;
    sink.flush()?;
    let base_size = snapshot.end();
    let write_ns = start.elapsed().as_nanos();
    drop(sink);
    // An append is a new committed generation: old bytes remain intact. This
    // measures a local Write sink, not an object-store conditional update.
    let append_start = Instant::now();
    let size = if append_entries == 0 {
        base_size
    } else {
        let file = OpenOptions::new().append(true).open(&args[1])?;
        let mut appended = Writer::append(BufWriter::new(file), &snapshot, base_size)?;
        for k in n..total {
            let mut value = [0; 16];
            value[..8].copy_from_slice(&k.to_le_bytes());
            value[8..].copy_from_slice(&(!k).to_le_bytes());
            let mut summary = Summary::default();
            if dimension_query {
                summary.width = 840;
                summary.height = 940;
            }
            if cicp_query {
                summary.cicp = Some(imij::Cicp::BT2100_PQ);
            }
            if filename_query {
                let mut metadata = EntryMetadata::default();
                metadata.filename = Some("other/image.imij".into());
                appended.insert_with_metadata(
                    k,
                    16,
                    0,
                    summary,
                    &metadata,
                    &mut value.as_slice(),
                )?;
            } else {
                appended.insert(k, 16, 0, summary, &mut value.as_slice())?;
            }
        }
        let (mut sink, snapshot) = appended.finish()?;
        sink.flush()?;
        snapshot.end()
    };
    let append_ns = append_start.elapsed().as_nanos();
    let batches = Rc::new(Cell::new(0));
    let bytes = Rc::new(Cell::new(0));
    let open_start = Instant::now();
    let mut r = Reader::open(Meter {
        inner: SeekReader::new(File::open(&args[1])?),
        batches: batches.clone(),
        bytes: bytes.clone(),
        latency: Duration::from_millis(latency_ms),
    })?;
    let open_ns = open_start.elapsed().as_nanos();
    let open_batches = batches.get();
    let open_bytes = bytes.get();
    let before = batches.get();
    let lookup_start = Instant::now();
    let e = r.get(n / 2)?.ok_or("missing key")?;
    let lookup_ns = lookup_start.elapsed().as_nanos();
    let lookup_batches = batches.get() - before;
    let lookup_bytes = bytes.get() - open_bytes;
    let copy_before = batches.get();
    let copy_byte_before = bytes.get();
    let copy_start = Instant::now();
    let mut value = Vec::new();
    r.copy_to(&e, &mut value)?;
    let copy_ns = copy_start.elapsed().as_nanos();
    let copy_batches = batches.get() - copy_before;
    let copy_bytes = bytes.get() - copy_byte_before;
    assert_eq!(&value[..8], &(n / 2).to_le_bytes());
    assert_eq!(&value[8..], &(!(n / 2)).to_le_bytes());
    let before = batches.get();
    let byte_before = bytes.get();
    let start = Instant::now();
    let mut list = r.listing()?;
    let mut count = 0;
    while let Some(e) = list.next(&mut r)? {
        assert_eq!(e.key(), count);
        assert_eq!(e.len(), 16);
        count += 1;
    }
    assert_eq!(count, total);
    let list_ns = start.elapsed().as_nanos();
    let list_batches = batches.get() - before;
    let list_bytes = bytes.get() - byte_before;
    let filtered = if dimension_query || cicp_query || filename_query {
        let before = batches.get();
        let byte_before = bytes.get();
        let start = Instant::now();
        let mut listing = r.listing()?;
        let mut matches = 0;
        while let Some(entry) = listing.next_matching(&mut r, |entry| {
            if dimension_query {
                entry.summary().width == 512 && entry.summary().height == 940
            } else if cicp_query {
                entry.summary().cicp == Some(imij::Cicp::SRGB)
            } else {
                entry.metadata().filename.as_deref() == Some("variants/longname-A.imij")
            }
        })? {
            assert_eq!(entry.key(), matches);
            matches += 1;
        }
        assert_eq!(matches, n.min(256));
        Some((
            matches,
            batches.get() - before,
            bytes.get() - byte_before,
            start.elapsed().as_nanos(),
        ))
    } else {
        None
    };
    let query = if dimension_query || cicp_query || filename_query {
        let before = batches.get();
        let byte_before = bytes.get();
        let start = Instant::now();
        let mut dimension_listing;
        let mut cicp_listing;
        let mut filename_listing;
        let mut matches = 0;
        if dimension_query {
            dimension_listing = r.dimension_listing(512..=512, 940..=940)?;
            while let Some(entry) = dimension_listing.next(&mut r)? {
                assert_eq!(entry.key(), matches);
                matches += 1;
            }
        } else if cicp_query {
            cicp_listing = r.cicp_listing(Some(imij::Cicp::SRGB))?;
            while let Some(entry) = cicp_listing.next(&mut r)? {
                assert_eq!(entry.key(), matches);
                matches += 1;
            }
        } else {
            filename_listing = r.filename_listing("variants/longname-A.imij")?;
            while let Some(entry) = filename_listing.next(&mut r)? {
                assert_eq!(entry.key(), matches);
                matches += 1;
            }
        }
        assert_eq!(matches, n.min(256));
        Some((
            matches,
            batches.get() - before,
            bytes.get() - byte_before,
            start.elapsed().as_nanos(),
        ))
    } else {
        None
    };
    if append_entries != 0 {
        let last = r.get(total - 1)?.ok_or("missing appended key")?;
        let mut last_value = Vec::new();
        r.copy_to(&last, &mut last_value)?;
        assert_eq!(&last_value[..8], &(total - 1).to_le_bytes());
        assert_eq!(&last_value[8..], &(!(total - 1)).to_le_bytes());
    }
    println!(
        "entries={total} base_entries={n} appended_entries={append_entries} blob_bytes={} base_archive_bytes={base_size} archive_bytes={size} index_bytes={} write_ns={write_ns} append_ns={append_ns} injected_batch_ms={latency_ms} open_batches={open_batches} open_bytes={open_bytes} open_ns={open_ns} lookup_batches={lookup_batches} lookup_bytes={lookup_bytes} lookup_ns={lookup_ns} copy_batches={copy_batches} copy_bytes={copy_bytes} copy_ns={copy_ns} list_batches={} list_bytes={} list_ns={}",
        total * 16,
        size - total * 16,
        list_batches,
        list_bytes,
        list_ns
    );
    if let Some((matches, batches, bytes, ns)) = query {
        println!(
            "{}_query matches={matches} batches={batches} bytes={bytes} ns={ns}",
            if dimension_query {
                "dimension"
            } else if cicp_query {
                "cicp"
            } else {
                "filename"
            }
        );
    }
    if let Some((matches, batches, bytes, ns)) = filtered {
        println!(
            "filtered_scan_{} matches={matches} batches={batches} bytes={bytes} ns={ns}",
            if dimension_query {
                "dimension"
            } else if cicp_query {
                "cicp"
            } else {
                "filename"
            }
        );
    }
    Ok(())
}

// Parse a frozen synthetic archive from memory. The same harness runs against
// both library revisions; file loading stays outside the timed operation.
fn read_existing(path: &str, expected: u64) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let scan = || -> Result<u64, Box<dyn std::error::Error>> {
        let mut reader = Reader::open(SliceReader::new(&bytes))?;
        let mut listing = reader.listing()?;
        let mut hash = zenhash::Xxh3::new();
        let mut count = 0;
        while let Some(entry) = listing.next(&mut reader)? {
            assert_eq!(entry.key(), count);
            assert_eq!(entry.len(), 16);
            hash.update(&entry.key().to_le_bytes());
            hash.update(entry.checksum().as_bytes());
            hash.update(&entry.metadata().to_bytes()?);
            count += 1;
        }
        assert_eq!(count, expected);
        Ok(hash.digest())
    };
    let expected_hash = scan()?;
    for _ in 0..2 {
        assert_eq!(scan()?, expected_hash);
    }
    let started = Instant::now();
    let actual = std::hint::black_box(scan()?);
    let elapsed = started.elapsed();
    assert_eq!(actual, expected_hash);
    println!("elapsed_ms={}", elapsed.as_secs_f64() * 1000.0);
    println!(
        "check={{\"entries\":{expected},\"archive_bytes\":{},\"listing_hash\":\"{actual:016x}\"}}",
        bytes.len()
    );
    Ok(())
}
