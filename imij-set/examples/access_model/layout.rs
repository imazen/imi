use super::input::Input;
use imij_set::{IndexPlacement, Reader, Writer};
use std::{
    fs::File,
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
};
use xxhash_rust::xxh3::Xxh3;

/// Flatten already-encoded level archives. Application catalog key bases are
/// returned explicitly; no native pyramid grammar is implied by this packing.
pub fn flatten(
    inputs: &[PathBuf],
    output: &Path,
    placement: IndexPlacement,
) -> io::Result<Vec<u64>> {
    let mut writer =
        Writer::new(BufWriter::new(File::create_new(output)?)).with_index_placement(placement)?;
    let mut bases = Vec::new();
    let mut base = 0u64;
    for input in inputs {
        bases.push(base);
        let mut reader = Reader::open(Input::open(
            input
                .to_str()
                .ok_or_else(|| io::Error::other("UTF-8 path"))?,
        )?)?;
        let mut listing = reader.listing()?;
        let mut next = base;
        while let Some(entry) = listing.next(&mut reader)? {
            let key = base
                .checked_add(entry.key())
                .ok_or_else(|| io::Error::other("flat key overflow"))?;
            next = key
                .checked_add(1)
                .ok_or_else(|| io::Error::other("flat key overflow"))?;
            writer.insert_stream_with_metadata(
                key,
                entry.head_size(),
                entry.summary(),
                entry.metadata(),
                |mut out| reader.copy_to(&entry, &mut out),
            )?;
        }
        base = next;
    }
    writer.finish()?.0.flush()?;
    let mut flat = Reader::open(Input::open(
        output
            .to_str()
            .ok_or_else(|| io::Error::other("UTF-8 path"))?,
    )?)?;
    for (input, base) in inputs.iter().zip(&bases) {
        let mut reader = Reader::open(Input::open(
            input
                .to_str()
                .ok_or_else(|| io::Error::other("UTF-8 path"))?,
        )?)?;
        let mut listing = reader.listing()?;
        while let Some(a) = listing.next(&mut reader)? {
            let b = flat
                .get(base + a.key())?
                .ok_or_else(|| io::Error::other("missing flat key"))?;
            if a.len() != b.len()
                || a.checksum() != b.checksum()
                || a.head_size() != b.head_size()
                || a.summary() != b.summary()
                || a.metadata() != b.metadata()
            {
                return Err(io::Error::other("flatten changed entry"));
            }
            flat.copy_to(&b, &mut io::sink())?;
        }
    }
    Ok(bases)
}

/// Streaming copy through the real checksum-checking archive reader/writer.
pub fn repack(
    input: &str,
    output: &Path,
    placement: IndexPlacement,
    inventory: &mut impl Write,
) -> io::Result<()> {
    let mut reader = Reader::open(Input::open(input)?)?;
    let mut writer =
        Writer::new(BufWriter::new(File::create_new(output)?)).with_index_placement(placement)?;
    let mut listing = reader.listing()?;
    while let Some(entry) = listing.next(&mut reader)? {
        writer.insert_stream_with_metadata(
            entry.key(),
            entry.head_size(),
            entry.summary(),
            entry.metadata(),
            |mut out| reader.copy_to(&entry, &mut out),
        )?;
        writeln!(
            inventory,
            "{input}\t{}\t{}\t{:016x}",
            entry.key(),
            entry.len(),
            entry.checksum()
        )?;
    }
    writer.finish()?.0.flush()?;
    // Compare original and rebuilt blobs, key order, metadata and summaries independently
    // of a later demand trace, so unvisited entries are not silently dropped.
    let mut rebuilt = Reader::open(Input::open(
        output
            .to_str()
            .ok_or_else(|| io::Error::other("UTF-8 path"))?,
    )?)?;
    let mut left = reader.listing()?;
    let mut right = rebuilt.listing()?;
    loop {
        match (left.next(&mut reader)?, right.next(&mut rebuilt)?) {
            (None, None) => break,
            (Some(a), Some(b))
                if a.key() == b.key()
                    && a.len() == b.len()
                    && a.checksum() == b.checksum()
                    && a.head_size() == b.head_size()
                    && a.summary() == b.summary()
                    && a.metadata() == b.metadata() =>
            {
                // Both complete blob digests, not just index declarations.
                reader.copy_to(&a, &mut io::sink())?;
                rebuilt.copy_to(&b, &mut io::sink())?;
            }
            _ => return Err(io::Error::other("repack changed entries")),
        }
    }
    Ok(())
}

/// Physically split, then read and hash all parts through the same replay adapter.
pub fn split(path: &Path, size: u64) -> io::Result<PathBuf> {
    if size == 0 {
        return Ok(path.to_owned());
    }
    let mut source = File::open(path)?;
    let length = source.metadata()?.len();
    if length == 0 || length.div_ceil(size) > 65536 {
        return Err(io::Error::other("part count/size"));
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::other("UTF-8 path"))?;
    let manifest_path = path.with_file_name(format!("{name}.parts.tsv"));
    let mut manifest = BufWriter::new(File::create_new(&manifest_path)?);
    writeln!(manifest, "imi-access-parts-v1\t{size}\t{length}")?;
    let mut buffer = [0; 65536];
    for i in 0..length.div_ceil(size) {
        let part_name = format!("{name}.part-{i:06}");
        let mut part = BufWriter::new(File::create_new(path.with_file_name(&part_name))?);
        let part_length = (length - i * size).min(size);
        let mut remaining = part_length;
        let mut hash = Xxh3::new();
        while remaining > 0 {
            let n = remaining.min(buffer.len() as u64) as usize;
            source.read_exact(&mut buffer[..n])?;
            part.write_all(&buffer[..n])?;
            hash.update(&buffer[..n]);
            remaining -= n as u64;
        }
        part.flush()?;
        writeln!(
            manifest,
            "{part_name}\t{part_length}\t{:016x}",
            hash.digest()
        )?;
    }
    manifest.flush()?;
    Input::open(
        manifest_path
            .to_str()
            .ok_or_else(|| io::Error::other("UTF-8 path"))?,
    )?
    .verify()?;
    Ok(manifest_path)
}
