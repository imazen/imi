//! Repackage a frozen demand trace, preserving every encoded image and its metadata.
#[path = "access_model/input.rs"]
mod input;
#[path = "access_model/layout.rs"]
mod layout;
use imij_set::{EntryMetadata, IndexPlacement, Summary, Writer};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() == 3 && args[0] == "split" {
        let size = args[2].parse()?;
        println!(
            "{}",
            layout::split(std::path::Path::new(&args[1]), size)?.display()
        );
        return Ok(());
    }
    if args.len() != 5 {
        return Err("access_layout TRACE OUTPUT_DIR interleaved|commit separate|packed|flat PART_BYTES (0=unsplit)".into());
    }
    let placement = match args[2].as_str() {
        "interleaved" => IndexPlacement::Interleaved,
        "commit" => IndexPlacement::AtCommit,
        _ => return Err("index placement".into()),
    };
    let organization = match args[3].as_str() {
        "separate" | "packed" | "flat" => args[3].as_str(),
        _ => return Err("organization".into()),
    };
    let part_size: u64 = args[4].parse()?;
    let out = PathBuf::from(&args[1]);
    std::fs::create_dir(&out)?;
    let out = out.canonicalize()?;
    let trace = std::fs::read_to_string(&args[0])?;
    let rows = trace
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_owned).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    if rows.is_empty() || rows.iter().any(|r| r.len() != 9 || r[3] != "archive") {
        return Err("layout preparation requires nonempty separate-archive trace".into());
    }
    let mut files = BTreeMap::new();
    for row in &rows {
        files.insert(row[2].clone(), 0usize);
    }
    let mut identity = BufWriter::new(File::create_new(out.join("entries.tsv"))?);
    writeln!(identity, "source\tkey\tbytes\txxh3")?;
    let mut levels = Vec::new();
    for (i, (original, id)) in files.iter_mut().enumerate() {
        *id = i;
        let level = out.join(format!("level-{i}.imiset"));
        layout::repack(original, &level, placement, &mut identity)?;
        levels.push(level);
    }
    identity.flush()?;
    let mut paths = Vec::new();
    let mut bases = Vec::new();
    if organization == "flat" {
        let path = out.join("flat.imiset");
        bases = layout::flatten(&levels, &path, placement)?;
        let mut catalog = BufWriter::new(File::create_new(out.join("key-bases.tsv"))?);
        writeln!(catalog, "source\tbase")?;
        for (source, id) in &files {
            writeln!(catalog, "{source}\t{}", bases[*id])?;
        }
        catalog.flush()?;
        paths.push(layout::split(&path, part_size)?);
    } else if organization == "packed" {
        let path = out.join("pyramid.imiset");
        let mut writer = Writer::new(BufWriter::new(File::create_new(&path)?))
            .with_index_placement(placement)?;
        for (i, level) in levels.iter().enumerate() {
            let mut file = File::open(level)?;
            let mut metadata = EntryMetadata::default();
            metadata.filename = Some(format!("level-{i}.imiset"));
            writer.insert_with_metadata(
                i as u64,
                file.metadata()?.len(),
                0,
                Summary::default(),
                &metadata,
                &mut file,
            )?;
        }
        writer.finish()?.0.flush()?;
        paths.push(layout::split(&path, part_size)?);
    } else {
        for level in levels {
            paths.push(layout::split(&level, part_size)?);
        }
    }
    let mut replay = BufWriter::new(File::create_new(out.join("replay.tsv"))?);
    for mut row in rows {
        let id = files[&row[2]];
        row[2] = paths[if organization != "separate" { 0 } else { id }]
            .to_str()
            .ok_or("UTF-8 path")?
            .to_owned();
        if organization == "packed" {
            row[3] = format!("archive:{id}");
        } else if organization == "flat" && row[4] != "-" {
            row[4] = row[4]
                .split(',')
                .map(|k| {
                    let key: u64 = k.parse()?;
                    bases[id]
                        .checked_add(key)
                        .map(|k| k.to_string())
                        .ok_or_else(|| "flat key overflow".into())
                })
                .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?
                .join(",");
        }
        writeln!(replay, "{}", row.join("\t"))?;
    }
    replay.flush()?;
    Ok(())
}
