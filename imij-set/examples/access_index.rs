//! Index-only scale experiment. Synthetic 16-byte blobs are not image timings.
#[path = "access_model/transport.rs"]
#[allow(dead_code)]
mod transport;
use imij::planes::SeekReader;
use imij_set::{IndexPlacement, Reader, Summary, Writer};
use std::{cell::RefCell, fs::File, io::Write, path::Path, rc::Rc};
use transport::{Config, Ledger, Transport};

fn value(key: u64) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&key.to_le_bytes());
    bytes[8..].copy_from_slice(&(!key).to_le_bytes());
    bytes
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return Err("access_index FRESH_OUTPUT_DIR".into());
    }
    let out = Path::new(&args[0]);
    std::fs::create_dir(out)?;
    let mut report = File::create_new(out.join("results.tsv"))?;
    writeln!(
        report,
        "count\tplacement\tfile_bytes\tindex_bytes\tquery\tkeys\tlookup_bytes\tlookup_rounds\tlookup_requests\tlookup_ms"
    )?;
    for count in [1u64, 256, 16384, 65536, 1_000_000] {
        for (name, placement) in [
            ("interleaved", IndexPlacement::Interleaved),
            ("commit", IndexPlacement::AtCommit),
        ] {
            let path = out.join(format!("{count}-{name}.imiset"));
            let mut writer = Writer::new(std::io::BufWriter::new(File::create_new(&path)?))
                .with_index_placement(placement)?;
            for key in 0..count {
                writer.insert(key, 16, 0, Summary::default(), &mut value(key).as_slice())?;
            }
            writer.finish()?.0.flush()?;
            let size = std::fs::metadata(&path)?.len();
            let width = count.isqrt().max(1);
            for (label, first) in [("first", 0), ("center", count / 2), ("last", count - 1)] {
                let mut keys = Vec::new();
                for row in 0..16 {
                    for col in 0..16 {
                        let key = first + row * width + col;
                        if key < count {
                            keys.push(key);
                        }
                    }
                }
                keys.sort_unstable();
                keys.dedup();
                let ledger = Rc::new(RefCell::new(Ledger::default()));
                let source = Transport::new(
                    SeekReader::new(File::open(&path)?),
                    name.into(),
                    Config {
                        ranges: true,
                        cache_bytes: 32 * 1024 * 1024,
                        latency_ms: 160.,
                        bytes_per_ms: 12500.,
                        concurrency: 8,
                        merge_gap: 4096,
                        max_range: 1024 * 1024,
                        part_size: 0,
                        shared_cache: false,
                    },
                    ledger.clone(),
                )?;
                let mut reader = Reader::open(source)?;
                let entries = reader.get_many(&keys)?;
                if entries.len() != keys.len() {
                    return Err("index query domain".into());
                }
                let l = ledger.borrow();
                writeln!(
                    report,
                    "{count}\t{name}\t{size}\t{}\t{label}\t{}\t{}\t{}\t{}\t{}",
                    size - count * 16,
                    keys.len(),
                    l.transferred_bytes,
                    l.rounds,
                    l.events.iter().map(|e| e.ranges.len()).sum::<usize>(),
                    l.now_ms
                )?;
                drop(l);
                // Validate payload contents too, outside the index-only accounting.
                for (key, entry) in keys.iter().zip(entries) {
                    let entry = entry.ok_or("index key missing")?;
                    if entry.key() != *key || entry.len() != 16 {
                        return Err("index entry identity".into());
                    }
                    let mut bytes = Vec::new();
                    reader.copy_to(&entry, &mut bytes)?;
                    if bytes != value(*key) {
                        return Err("index payload identity".into());
                    }
                }
            }
            println!("{count} {name} PASS");
        }
    }
    Ok(())
}
