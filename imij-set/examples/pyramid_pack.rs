//! Package existing pyramid levels, indexes and a catalog without re-encoding.
use imij::planes::SeekReader;
use imij_set::{EntryMetadata, Reader, Summary, Writer};
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read};

fn pack<W: io::Write, R: Read>(sink: W, inputs: &mut [(String, u64, R)]) -> io::Result<W> {
    let mut writer = Writer::new(sink);
    for (key, (name, length, source)) in inputs.iter_mut().enumerate() {
        let mut metadata = EntryMetadata::default();
        metadata.filename = Some(name.clone());
        writer.insert_with_metadata(
            key as u64,
            *length,
            0,
            Summary::default(),
            &metadata,
            source,
        )?;
    }
    Ok(writer.finish()?.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .ok_or("usage: pyramid_pack OUTPUT NAME=PATH ...")?;
    let mut inputs = Vec::new();
    let mut names = BTreeSet::new();
    for arg in args {
        let (name, path) = arg.split_once('=').ok_or("expected NAME=PATH")?;
        if name.is_empty()
            || name.starts_with('/')
            || name.chars().any(char::is_control)
            || name.contains('\\')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !names.insert(name.to_owned())
        {
            return Err("names must be distinct relative paths without control characters".into());
        }
        let file = File::open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err("inputs must be regular files".into());
        }
        inputs.push((name.to_owned(), meta.len(), file));
    }
    if inputs.is_empty() {
        return Err("at least one input is required".into());
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    pack(BufWriter::new(file), &mut inputs)?;
    let mut reader = Reader::open(SeekReader::new(File::open(output)?))?;
    let mut listing = reader.listing()?;
    println!("key\toffset\tlength\txxh3\tname");
    while let Some(entry) = listing.next(&mut reader)? {
        println!(
            "{}\t{}\t{}\t{:016x}\t{}",
            entry.key(),
            entry.offset(),
            entry.len(),
            entry.checksum(),
            entry.metadata().filename.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use imij::planes::SliceReader;
    use std::io::Cursor;

    #[test]
    fn levels_remain_byte_identical_and_independently_addressable() {
        let mut inputs = Vec::new();
        for (level, body) in [b"full pixels".as_slice(), b"half pixels".as_slice()]
            .into_iter()
            .enumerate()
        {
            let mut writer = Writer::new(Vec::new());
            writer
                .insert(42, body.len() as u64, 0, Summary::default(), &mut &body[..])
                .unwrap();
            let (bytes, _) = writer.finish().unwrap();
            inputs.push((
                format!("lossless/{level}.imijset"),
                bytes.len() as u64,
                Cursor::new(bytes),
            ));
        }
        let bytes = pack(Vec::new(), &mut inputs).unwrap();
        let mut reader = Reader::open(SliceReader::new(&bytes)).unwrap();
        for (key, (name, _, original)) in inputs.iter().enumerate() {
            let entry = reader.get(key as u64).unwrap().unwrap();
            assert_eq!(entry.metadata().filename.as_deref(), Some(name.as_str()));
            assert_eq!(
                reader
                    .read_entry_bounded(&entry, original.get_ref().len())
                    .unwrap(),
                *original.get_ref()
            );
            let source = reader.entry_source(&entry).unwrap();
            let mut level = Reader::open(source).unwrap();
            let tile = level.get(42).unwrap().unwrap();
            let expected = if key == 0 {
                b"full pixels"
            } else {
                b"half pixels"
            };
            assert_eq!(
                level.read_entry_bounded(&tile, expected.len()).unwrap(),
                expected
            );
        }
    }
}
