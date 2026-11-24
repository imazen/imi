//! Pack a prepared archive entry plan while retaining each input blob byte for byte.
use imij_set::{EntryMetadata, Summary, Value, Writer};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn number<T: std::str::FromStr>(value: &str) -> io::Result<T> {
    value
        .parse()
        .map_err(|_| invalid("invalid entry plan number"))
}

fn main() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let plan = args
        .next()
        .ok_or_else(|| invalid("usage: browser_pack PLAN.tsv OUTPUT"))?;
    let output = args
        .next()
        .ok_or_else(|| invalid("usage: browser_pack PLAN.tsv OUTPUT"))?;
    if args.next().is_some() {
        return Err(invalid("usage: browser_pack PLAN.tsv OUTPUT"));
    }
    let plan = BufReader::new(File::open(plan)?);
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)?;
    let mut writer = Writer::new(BufWriter::new(file));
    for line in plan.lines() {
        let line = line?;
        let fields: Vec<_> = line.split('\t').collect();
        let [key, name, media, profile, width, height, path] = fields.as_slice() else {
            return Err(invalid(
                "entry plan needs key, name, media, profile, width, height and path",
            ));
        };
        let mut meta = EntryMetadata::default();
        meta.filename = Some((*name).into());
        if !media.is_empty() {
            meta.attributes
                .insert("media_type".into(), Value::Text((*media).into()));
        }
        if !profile.is_empty() {
            meta.attributes
                .insert("imij.profile".into(), Value::Text((*profile).into()));
        }
        let mut summary = Summary::default();
        summary.width = number(width)?;
        summary.height = number(height)?;
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        writer.insert_with_metadata(
            number(key)?,
            length,
            0,
            summary,
            &meta,
            &mut BufReader::new(file),
        )?;
    }
    let (mut sink, _) = writer.finish()?;
    sink.flush()
}
