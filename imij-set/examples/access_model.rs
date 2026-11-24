//! Replay a TSV demand trace through production archive/region readers.
#[path = "access_model/input.rs"]
mod input;
#[path = "access_model/selection.rs"]
mod selection;
#[path = "access_model/source.rs"]
mod source;
#[path = "access_model/transport.rs"]
mod transport;
use imij::{
    planes::{ReadAt, Rect, SliceReader},
    region::RegionReader,
};
use input::Input;
use source::{ArchiveTree, Window};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    rc::Rc,
};
use transport::{Config, Ledger, SharedSource, Transport};
use xxhash_rust::xxh3::Xxh3;

type Source = SharedSource<Input>;
type Failure = Box<dyn std::error::Error>;
fn output(path: &str) -> Result<File, Failure> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}
fn pixels<R: imij::planes::ReadAt>(
    reader: &mut RegionReader<R>,
    rect: Rect,
) -> Result<Vec<u8>, Failure> {
    let pixel_bytes = reader.info().channels.count() * reader.info().sample.bytes();
    let stride = (rect.width() as usize)
        .checked_mul(pixel_bytes)
        .ok_or("stride overflow")?;
    let length = stride
        .checked_mul(rect.height() as usize)
        .filter(|n| *n <= 64 * 1024 * 1024)
        .ok_or("ROI exceeds explicit 64 MiB replay limit")?;
    let mut bytes = vec![0; length];
    reader.decode_region(rect, &mut bytes, stride)?;
    Ok(bytes)
}
fn main() -> Result<(), Failure> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(8..=11).contains(&args.len()) {
        return Err("access_model TRACE.tsv OUTPUT_PREFIX ranges|whole CACHE_BYTES RTT_MS BYTES_PER_SECOND CONCURRENCY serial|batch [object|shared [MERGE_GAP [MAX_RANGE]]]".into());
    }
    let batched = match args[7].as_str() {
        "serial" => false,
        "batch" => true,
        _ => return Err("serial|batch".into()),
    };
    let ranges = match args[2].as_str() {
        "ranges" => true,
        "whole" => false,
        _ => return Err("ranges|whole".into()),
    };
    let cfg = Config {
        ranges,
        cache_bytes: args[3].parse()?,
        latency_ms: args[4].parse()?,
        bytes_per_ms: args[5].parse::<f64>()? / 1000.0,
        concurrency: args[6].parse()?,
        merge_gap: args.get(9).map(|s| s.parse()).transpose()?.unwrap_or(4096),
        max_range: args
            .get(10)
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(1024 * 1024),
        part_size: 0,
        shared_cache: match args.get(8).map(String::as_str).unwrap_or("object") {
            "object" => false,
            "shared" => true,
            _ => return Err("cache scope object|shared".into()),
        },
    }
    .validate()?;
    let mut report = output(&format!("{}.views.tsv", args[1]))?;
    let mut events = output(&format!("{}.ranges.tsv", args[1]))?;
    let mut ready = output(&format!("{}.entries.tsv", args[1]))?;
    writeln!(ready, "label\tkey\tdata_ready_ms\tdecoded_bytes\txxh3")?;
    writeln!(
        report,
        "label\tarrival_ms\tstart_ms\tfinish_ms\trequests\tdependency_rounds\tbytes\thit_bytes\tdecoded_bytes\txxh3\tranges\tcache_budget\trtt_ms\tbytes_per_second\tconcurrency\tcache_scope\tpart_size\tmerge_gap\tmax_range"
    )?;
    writeln!(
        events,
        "label\tobject\tround\tstart_ms\tfinish_ms\tstatus\tstart\tend_exclusive\tlogical_start\tlogical_end\tobject_length"
    )?;
    let ledger = Rc::new(RefCell::new(Ledger::default()));
    let mut archives: BTreeMap<String, ArchiveTree<Source>> = BTreeMap::new();
    let mut references: BTreeMap<String, ArchiveTree<Source>> = BTreeMap::new();
    let mut images: BTreeMap<(String, Vec<u64>), RegionReader<Window<Source>>> = BTreeMap::new();
    let mut pyramids: BTreeMap<(String, Vec<u64>), selection::PyramidReader<Window<Source>>> =
        BTreeMap::new();
    let mut identities = BTreeMap::new();
    let mut previous = 0.0;
    for line in BufReader::new(File::open(&args[0])?).lines() {
        let line = line?;
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<_> = line.split('\t').collect();
        if f.len() != 9 {
            return Err("trace requires label,time_ms,file,kind,keys,x,y,width,height".into());
        }
        let at: f64 = f[1].parse()?;
        if !at.is_finite() || at < previous {
            return Err("nonmonotonic trace time".into());
        }
        previous = at;
        let (start, bytes0, hits0, round0, event0) = {
            let mut l = ledger.borrow_mut();
            l.now_ms = l.now_ms.max(at);
            (
                l.now_ms,
                l.transferred_bytes,
                l.cache_hit_bytes,
                l.rounds,
                l.events.len(),
            )
        };
        let (kind, path) = f[3].split_once(':').unwrap_or((f[3], ""));
        let path = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/')
                .map(str::parse)
                .collect::<Result<Vec<u64>, _>>()?
        };
        if !archives.contains_key(f[2]) {
            let input = Input::open(f[2])?;
            let mut config = cfg;
            config.part_size = input.part_size;
            identities.insert(f[2].to_owned(), input.clone());
            let source = SharedSource::new(Transport::new(
                input.clone(),
                f[2].to_owned(),
                config,
                ledger.clone(),
            )?);
            archives.insert(f[2].to_owned(), ArchiveTree::new(source)?);
            let direct = SharedSource::new(Transport::new(
                input,
                f[2].to_owned(),
                config,
                Rc::new(RefCell::new(Ledger::default())),
            )?);
            references.insert(f[2].to_owned(), ArchiveTree::new(direct)?);
        }
        let tree = archives.get_mut(f[2]).unwrap();
        let reference = references.get_mut(f[2]).unwrap();
        let mut hash = Xxh3::new();
        let mut decoded = 0;
        match kind {
            "archive" => {
                let mut source = tree.source(&path)?;
                let reader = tree.reader(&path)?;
                let keys = if f[4] == "-" {
                    Vec::new()
                } else {
                    f[4].split(',')
                        .map(str::parse)
                        .collect::<Result<Vec<u64>, _>>()?
                };
                if keys.len() > 4096 {
                    return Err("trace exceeds 4096 keys/view".into());
                }
                let entries = reader
                    .get_many(&keys)?
                    .into_iter()
                    .map(|e| e.ok_or("missing archive key"))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut cursor = 0;
                while cursor < entries.len() {
                    let start = cursor;
                    let mut size = 0;
                    while cursor < entries.len() {
                        let n = usize::try_from(entries[cursor].len())?;
                        if batched && n > 16 * 1024 * 1024 {
                            return Err("single entry exceeds 16 MiB batch limit".into());
                        }
                        if cursor > start && (!batched || size + n > 16 * 1024 * 1024) {
                            break;
                        }
                        size += n;
                        cursor += 1;
                    }
                    let batch = &entries[start..cursor];
                    let mut buffers = if batched {
                        batch
                            .iter()
                            .map(|e| vec![0; e.len() as usize])
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    if batched {
                        let mut requests = batch
                            .iter()
                            .zip(&mut buffers)
                            .map(|(e, b)| (e.offset(), b.as_mut_slice()))
                            .collect::<Vec<_>>();
                        source.read_many_at(&mut requests)?;
                    }
                    for (i, entry) in batch.iter().enumerate() {
                        let (out, rect) = if batched {
                            if imij::HashAlgorithm::DEFAULT.digest(&buffers[i]) != entry.checksum()
                            {
                                return Err("batched entry checksum mismatch".into());
                            }
                            let mut region = RegionReader::open(SliceReader::new(&buffers[i]))?;
                            let rect = Rect::new(0, 0, region.info().width, region.info().height);
                            (pixels(&mut region, rect)?, rect)
                        } else {
                            let mut region = RegionReader::open(reader.entry_source(entry)?)?;
                            let rect = Rect::new(0, 0, region.info().width, region.info().height);
                            (pixels(&mut region, rect)?, rect)
                        };
                        // Direct immutable-file reader, outside modeled transport.
                        let reference = reference.reader(&path)?;
                        let e = reference.get(entry.key())?.ok_or("missing reference key")?;
                        let mut rr = RegionReader::open(reference.entry_source(&e)?)?;
                        if out != pixels(&mut rr, rect)? {
                            return Err("modeled transport changed decoded pixels".into());
                        }
                        // This clock contains actual reader dependencies with modeled
                        // transport costs, but no CPU duration. It is data readiness,
                        // not a claim that the browser has displayed these pixels.
                        writeln!(
                            ready,
                            "{}\t{}\t{}\t{}\t{:016x}",
                            f[0],
                            entry.key(),
                            ledger.borrow().now_ms,
                            out.len(),
                            xxhash_rust::xxh3::xxh3_64(&out)
                        )?;
                        decoded += out.len();
                        hash.update(&out);
                    }
                }
            }
            "image" => {
                let image_key = (f[2].to_owned(), path.clone());
                if !images.contains_key(&image_key) {
                    images.insert(image_key.clone(), RegionReader::open(tree.source(&path)?)?);
                }
                let rect = Rect::new(f[5].parse()?, f[6].parse()?, f[7].parse()?, f[8].parse()?);
                let reader = images.get_mut(&image_key).unwrap();
                let _plan = reader.region_ranges(rect)?;
                let out = pixels(reader, rect)?;
                let mut rr = RegionReader::open(reference.source(&path)?)?;
                if out != pixels(&mut rr, rect)? {
                    return Err("modeled transport changed decoded pixels".into());
                }
                writeln!(
                    ready,
                    "{}\t-\t{}\t{}\t{:016x}",
                    f[0],
                    ledger.borrow().now_ms,
                    out.len(),
                    xxhash_rust::xxh3::xxh3_64(&out)
                )?;
                decoded += out.len();
                hash.update(&out);
            }
            "pyramid" => {
                let image_key = (f[2].to_owned(), path.clone());
                if !pyramids.contains_key(&image_key) {
                    pyramids.insert(
                        image_key.clone(),
                        selection::PyramidReader::open(
                            tree.source(&path)?,
                            selection::Limits::default(),
                        )?,
                    );
                }
                let selected = selection::Selection {
                    level: f[4].parse()?,
                    rect: Rect::new(f[5].parse()?, f[6].parse()?, f[7].parse()?, f[8].parse()?),
                };
                let delivery = if batched {
                    selection::Delivery::WholeEntries
                } else {
                    selection::Delivery::NativeRanges
                };
                // Input is the existing immutable fixture source. The live HTTP
                // adapter must enforce its version during reads; this replay
                // retains the existing final Input::verify audit as well.
                let out =
                    pyramids
                        .get_mut(&image_key)
                        .unwrap()
                        .decode(selected, delivery, || Ok(()))?;
                let mut direct = selection::PyramidReader::open(
                    reference.source(&path)?,
                    selection::Limits::default(),
                )?;
                let expected =
                    direct.decode(selected, selection::Delivery::NativeRanges, || Ok(()))?;
                if out.bytes != expected.bytes
                    || out.info != expected.info
                    || out.orientation != expected.orientation
                {
                    return Err(
                        "modeled spatial transport changed stored samples or header facts".into(),
                    );
                }
                writeln!(
                    ready,
                    "{}\tlevel:{}\t{}\t{}\t{:016x}",
                    f[0],
                    selected.level,
                    ledger.borrow().now_ms,
                    out.bytes.len(),
                    xxhash_rust::xxh3::xxh3_64(&out.bytes)
                )?;
                decoded += out.bytes.len();
                hash.update(&out.bytes);
            }
            _ => return Err("trace kind must be image, archive or pyramid".into()),
        }
        let l = ledger.borrow();
        let new = &l.events[event0..];
        let requests: usize = new.iter().map(|e| e.ranges.len()).sum();
        writeln!(
            report,
            "{}\t{at}\t{start}\t{}\t{requests}\t{}\t{}\t{}\t{decoded}\t{:016x}\t{ranges}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            f[0],
            l.now_ms,
            l.rounds - round0,
            l.transferred_bytes - bytes0,
            l.cache_hit_bytes - hits0,
            hash.digest(),
            cfg.cache_bytes,
            cfg.latency_ms,
            cfg.bytes_per_ms * 1000.0,
            cfg.concurrency,
            if cfg.shared_cache { "shared" } else { "object" },
            identities[f[2]].part_size,
            cfg.merge_gap,
            cfg.max_range
        )?;
        for e in new {
            for r in &e.ranges {
                writeln!(
                    events,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    f[0],
                    e.object,
                    e.round,
                    e.start_ms,
                    e.end_ms,
                    e.status,
                    r.start,
                    r.end,
                    e.logical_base + r.start,
                    e.logical_base + r.end,
                    e.object_length
                )?;
            }
        }
    }
    for input in identities.values() {
        input.verify()?;
    }
    Ok(())
}
