//! Compiled only in the prepared PR208 graph, not a new production API.
use imij::planes::ReadAt as LegacyReadAt;
use imij_set::Reader;
use std::{cell::RefCell, error::Error, rc::Rc};
use zencodec::source::{DynReadAt, SliceReadAt};
use zencodec_io::FileReadAt;

#[derive(Default)]
struct Counts {
    hints: usize,
    lengths: usize,
    reads: usize,
    batches: Vec<usize>,
}
struct Bridge<'a> {
    source: Box<dyn DynReadAt + 'a>,
    counts: Rc<RefCell<Counts>>,
}
impl LegacyReadAt for Bridge<'_> {
    fn len(&mut self) -> Result<u64, imij::Error> {
        if let Some(n) = self.source.len_hint() {
            self.counts.borrow_mut().hints += 1;
            return Ok(n);
        }
        self.counts.borrow_mut().lengths += 1;
        self.source.len().map_err(|_| imij::Error::Read)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.counts.borrow_mut().reads += 1;
        self.source
            .read_exact_at(at, out)
            .map_err(|_| imij::Error::Read)
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
        self.counts.borrow_mut().batches.push(reqs.len());
        self.source
            .read_many_at(reqs)
            .map_err(|_| imij::Error::Read)
    }
}
fn exercise(
    source: Box<dyn DynReadAt + '_>,
    keys: &[u64],
    backend: &str,
) -> Result<Vec<u128>, Box<dyn Error>> {
    let counts = Rc::new(RefCell::new(Counts::default()));
    let mut reader = Reader::open(Bridge {
        source,
        counts: Rc::clone(&counts),
    })?;
    let entries = reader.get_many(keys)?;
    let mut identities = Vec::new();
    for entry in entries {
        let entry = entry.ok_or("missing probe key")?;
        let encoded = reader.read_entry_bounded(&entry, 64 * 1024 * 1024)?;
        let (info, pixels) = imij::decode(&encoded)?;
        let hash = xxhash_rust::xxh3::xxh3_128(&pixels);
        identities.push(hash);
        println!(
            "{backend}\t{}\t{}\t{}\t{hash:032x}",
            entry.key(),
            info.width,
            info.height
        );
    }
    let c = counts.borrow();
    eprintln!(
        "source={backend} hints={} lengths={} reads={} batches={:?}",
        c.hints, c.lengths, c.reads, c.batches
    );
    Ok(identities)
}
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = args.first().ok_or("ARCHIVE KEY...")?;
    let keys: Vec<u64> = args[1..]
        .iter()
        .map(|s| s.parse())
        .collect::<Result<_, _>>()?;
    if keys.is_empty() || std::fs::metadata(path)?.len() > 64 * 1024 * 1024 {
        return Err("bounded real-file probe required".into());
    }
    let file = exercise(Box::from(FileReadAt::open(path)?), &keys, "file")?;
    let bytes = std::fs::read(path)?;
    let slice = exercise(Box::from(SliceReadAt::new(&bytes)), &keys, "slice")?;
    if file != slice {
        return Err("source backend pixels disagree".into());
    }
    // The legacy trait cannot expose as_slice or preserve the rich error type.
    // This is a compatibility probe, not complete production source migration.
    Ok(())
}
