//! Experimental dense ordinal directory, not an IMI/imij-set wire extension.
//! Compare placement separately from paging using byte-identical opaque payloads.
#[path = "access_model/transport.rs"]
#[allow(dead_code)]
mod transport;
use imij::planes::{ReadAt, SeekReader};
use imij_set::Reader;
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    rc::Rc,
};
use transport::{Config, Ledger, Transport};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const PAGE: usize = 32 + 256 * 12;
const HEAD: usize = 64;
fn word(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn put(b: &mut [u8], at: usize, n: u64) {
    b[at..at + 8].copy_from_slice(&n.to_le_bytes());
}
fn hash(b: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(b)
}
fn parse_page(
    b: &[u8],
    ordinal: u64,
    count: u64,
    low: u64,
    high: u64,
) -> Result<Vec<(u64, u64, u64)>> {
    if b.len() != PAGE || hash(&b[..PAGE - 8]) != word(b, PAGE - 8) || word(b, 0) != ordinal {
        return Err("dense page identity/hash".into());
    }
    let first = ordinal
        .checked_mul(256)
        .filter(|&v| v < count)
        .ok_or("dense ordinal bounds")?;
    let n = (count - first).min(256) as usize;
    if word(b, 16) != n as u64 || b[24 + n * 12..PAGE - 8].iter().any(|&v| v != 0) {
        return Err("dense page count/padding".into());
    }
    let mut at = word(b, 8);
    let mut result = Vec::with_capacity(n);
    for i in 0..n {
        let p = 24 + i * 12;
        let len = u32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as u64;
        let end = at.checked_add(len).ok_or("dense extent overflow")?;
        if at < low || end > high {
            return Err("dense payload bounds".into());
        }
        result.push((at, len, word(b, p + 4)));
        at = end;
    }
    Ok(result)
}
fn config() -> Config {
    Config {
        ranges: true,
        cache_bytes: 32 << 20,
        latency_ms: 160.,
        bytes_per_ms: 12500.,
        concurrency: 8,
        merge_gap: 4096,
        max_range: 1 << 20,
        part_size: 0,
        shared_cache: false,
    }
}
fn main() -> Result<()> {
    let a = std::env::args().skip(1).collect::<Vec<_>>();
    if a.len() != 2 {
        return Err("access_dense INPUT.imiset FRESH_OUTPUT".into());
    }
    let out = Path::new(&a[1]);
    std::fs::create_dir(out)?;
    let mut original = Reader::open(SeekReader::new(File::open(&a[0])?))?;
    let mut entries = Vec::new();
    let mut listing = original.listing()?;
    while let Some(e) = listing.next(&mut original)? {
        if e.key() != entries.len() as u64
            || e.len() > u32::MAX as u64
            || entries.len() >= 1_000_000
        {
            return Err(
                "dense study requires at most one million dense keys and u32 lengths".into(),
            );
        }
        entries.push(e);
    }
    if entries.is_empty() {
        return Err("nonempty dense study".into());
    }
    let count = entries.len() as u64;
    let pages = count.div_ceil(256);
    let index_size = pages * PAGE as u64;
    let payload_size = entries.iter().map(|e| e.len()).sum::<u64>();
    let length = HEAD as u64 + index_size + payload_size;
    let mut report = File::create_new(out.join("results.tsv"))?;
    writeln!(
        report,
        "placement\tcount\tfile_bytes\tindex_bytes\tquery\tkeys\tlookup_bytes\tlookup_rounds\tlookup_requests\tlookup_ms"
    )?;
    for front in [false, true] {
        let name = if front { "front" } else { "tail" };
        let path = out.join(format!("{name}.lab"));
        let (header_at, index_at, data_at) = if front {
            (0, HEAD as u64, HEAD as u64 + index_size)
        } else {
            (payload_size + index_size, payload_size, 0)
        };
        let mut file = File::create_new(&path)?;
        file.set_len(length)?;
        let mut header = [0; HEAD];
        header[..8].copy_from_slice(b"IMILAB01");
        for (i, v) in [count, index_at, data_at, payload_size, length, PAGE as u64]
            .iter()
            .enumerate()
        {
            put(&mut header, 8 + i * 8, *v);
        }
        let digest = hash(&header[..56]);
        put(&mut header, 56, digest);
        file.seek(SeekFrom::Start(header_at))?;
        file.write_all(&header)?;
        let mut payload = data_at;
        for (ordinal, chunk) in entries.chunks(256).enumerate() {
            let mut page = vec![0; PAGE];
            put(&mut page, 0, ordinal as u64);
            put(&mut page, 8, payload);
            put(&mut page, 16, chunk.len() as u64);
            for (i, e) in chunk.iter().enumerate() {
                let p = 24 + i * 12;
                page[p..p + 4].copy_from_slice(&(e.len() as u32).to_le_bytes());
                put(&mut page, p + 4, e.checksum().low_u64());
                file.seek(SeekFrom::Start(payload))?;
                original.copy_to(e, &mut file)?;
                payload += e.len();
            }
            let digest = hash(&page[..PAGE - 8]);
            put(&mut page, PAGE - 8, digest);
            parse_page(
                &page,
                ordinal as u64,
                count,
                data_at,
                data_at + payload_size,
            )?;
            file.seek(SeekFrom::Start(index_at + ordinal as u64 * PAGE as u64))?;
            file.write_all(&page)?;
        }
        file.flush()?;
        drop(file);
        let width = count.isqrt().max(1);
        for (query, first) in [("first", 0), ("center", count / 2), ("last", count - 1)] {
            let keys = (0..16)
                .flat_map(|r| (0..16).map(move |c| first + r * width + c))
                .filter(|&k| k < count)
                .collect::<BTreeSet<_>>();
            let ledger = Rc::new(RefCell::new(Ledger::default()));
            let mut source = Transport::new(
                SeekReader::new(File::open(&path)?),
                name.into(),
                config(),
                ledger.clone(),
            )?;
            let read_len = length.min(65536) as usize;
            let start = if front { 0 } else { length - read_len as u64 };
            let mut bootstrap = vec![0; read_len];
            source.read_exact_at(start, &mut bootstrap)?;
            let h = &bootstrap[(header_at - start) as usize..][..HEAD];
            if &h[..8] != b"IMILAB01"
                || hash(&h[..56]) != word(h, 56)
                || word(h, 8) != count
                || word(h, 16) != index_at
                || word(h, 24) != data_at
                || word(h, 32) != payload_size
                || word(h, 40) != length
                || word(h, 48) != PAGE as u64
            {
                return Err("dense bootstrap identity".into());
            }
            let selected = keys.iter().map(|k| k / 256).collect::<BTreeSet<_>>();
            let mut buffers = selected
                .iter()
                .map(|&p| (p, vec![0; PAGE]))
                .collect::<Vec<_>>();
            let mut requests = Vec::new();
            for (p, b) in &mut buffers {
                let at = index_at + *p * PAGE as u64;
                if at >= start && at + PAGE as u64 <= start + bootstrap.len() as u64 {
                    b.copy_from_slice(&bootstrap[(at - start) as usize..][..PAGE]);
                } else {
                    requests.push((at, b.as_mut_slice()));
                }
            }
            if !requests.is_empty() {
                source.read_many_at(&mut requests)?;
            }
            let mut selected_entries = Vec::new();
            for (ordinal, b) in buffers {
                let values = parse_page(&b, ordinal, count, data_at, data_at + payload_size)?;
                for (i, (at, len, digest)) in values.into_iter().enumerate() {
                    let key = ordinal * 256 + i as u64;
                    if keys.contains(&key) {
                        if len != entries[key as usize].len()
                            || digest != entries[key as usize].checksum().low_u64()
                        {
                            return Err("dense entry changed".into());
                        }
                        selected_entries.push((at, len, digest));
                    }
                }
            }
            if selected_entries.len() != keys.len() {
                return Err("dense query domain".into());
            }
            let l = ledger.borrow();
            writeln!(
                report,
                "{name}\t{count}\t{length}\t{}\t{query}\t{}\t{}\t{}\t{}\t{}",
                index_size + HEAD as u64,
                keys.len(),
                l.transferred_bytes,
                l.rounds,
                l.events.iter().map(|e| e.ranges.len()).sum::<usize>(),
                l.now_ms
            )?;
            drop(l);
            // Independent streaming check of every selected payload; never in the model clock.
            let mut file = File::open(&path)?;
            for (at, len, digest) in selected_entries {
                file.seek(SeekFrom::Start(at))?;
                let mut remaining = len;
                let mut hash = imij::Hasher::new(imij::HashAlgorithm::DEFAULT);
                let mut buffer = [0; 65536];
                while remaining > 0 {
                    let n = remaining.min(buffer.len() as u64) as usize;
                    file.read_exact(&mut buffer[..n])?;
                    hash.update(&buffer[..n]);
                    remaining -= n as u64;
                }
                if hash.finish().low_u64() != digest {
                    return Err("dense payload checksum".into());
                }
            }
        }
        println!("{name}: {count} entries, bytes and lookup domains PASS");
    }
    Ok(())
}

#[test]
fn dense_page_rejects_wrong_ordinal_and_out_of_bounds() {
    let mut page = vec![0; PAGE];
    put(&mut page, 8, 100);
    put(&mut page, 16, 1);
    page[24..28].copy_from_slice(&16u32.to_le_bytes());
    put(&mut page, 28, hash(&[0; 16]));
    let h = hash(&page[..PAGE - 8]);
    put(&mut page, PAGE - 8, h);
    assert!(parse_page(&page, 0, 1, 100, 116).is_ok());
    assert!(parse_page(&page, 1, 257, 100, 116).is_err());
    assert!(parse_page(&page, 0, 1, 100, 115).is_err());
    page[40] = 1;
    let h = hash(&page[..PAGE - 8]);
    put(&mut page, PAGE - 8, h);
    assert!(parse_page(&page, 0, 1, 100, 116).is_err());
}
