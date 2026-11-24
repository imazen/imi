//! Rebuild one browser pyramid level from its lossless RGB8 archive.
//! The IMITILE2 accelerator is regenerated from the committed archive index.
use imij::planes::SeekReader;
use imij::{Channels, DecodeSpeed, ImageRef, ImiEncoderConfig, LossyQuality, SampleType};
use imij_set::{IndexPlacement, PyramidCatalog, PyramidLevel, Reader, Summary, TileOrder, Writer};
use rayon::prelude::*;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const WIDTH: u32 = 512;
const HEIGHT: u32 = 256;
const MAX_TILE: usize = 16 << 20;
const PAGE: usize = 256;

fn fresh(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn offsets(path: &Path, output: &Path) -> Result<()> {
    let mut archive = Reader::open(SeekReader::new(File::open(path)?))?;
    let entry = archive.get(0)?.ok_or("missing pyramid catalog")?;
    let catalog = PyramidCatalog::decode(&archive.read_entry_bounded(&entry, 64 << 10)?)?;
    let [level] = catalog.levels() else {
        return Err("expected one Explorer archive level".into());
    };
    if level.order != TileOrder::RowMajor
        || level.key_base != 1
        || (level.tile_width, level.tile_height) != (WIDTH, HEIGHT)
    {
        return Err("expected row-major Explorer tile layout".into());
    }
    let count = u32::try_from(level.tile_count())?;
    let mut entries = archive.listing_from(1)?;
    let mut out = BufWriter::new(fresh(output)?);
    out.write_all(b"IMITILE2")?;
    for field in [level.width, level.height, WIDTH, HEIGHT, count] {
        out.write_all(&field.to_le_bytes())?;
    }
    let mut end = None;
    let mut base = 0;
    for ordinal in 0..count {
        let entry = entries.next(&mut archive)?.ok_or("missing output tile")?;
        if entry.key() != u64::from(ordinal) + 1
            || end.is_some_and(|end| end != entry.offset())
            || entry.is_empty()
            || entry.len() > MAX_TILE as u64
        {
            return Err("output tiles are not a contiguous row-major sequence".into());
        }
        if (ordinal as usize).is_multiple_of(PAGE) {
            base = entry.offset();
            out.write_all(&base.to_le_bytes())?;
        }
        let tile_end = entry
            .offset()
            .checked_add(entry.len())
            .ok_or("tile extent")?;
        out.write_all(&u32::try_from(tile_end - base)?.to_le_bytes())?;
        end = Some(tile_end);
    }
    if entries.next(&mut archive)?.is_some() {
        return Err("extra output tiles".into());
    }
    out.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [mode, archive, output] = args.as_slice()
        && mode == "offsets"
    {
        return offsets(Path::new(archive), Path::new(output));
    }
    let [source, width, height, output, mode] = args.as_slice() else {
        return Err(
            "usage: reencode_tiles SOURCE WIDTH HEIGHT OUT.imijset lossless|QUALITY\n       reencode_tiles offsets ARCHIVE OUT.index".into(),
        );
    };
    let width: u32 = width.parse()?;
    let height: u32 = height.parse()?;
    let path = Path::new(output);
    let cols = width.div_ceil(WIDTH);
    let count = cols
        .checked_mul(height.div_ceil(HEIGHT))
        .ok_or("level extents")?;
    let quality = if mode == "lossless" {
        None
    } else {
        Some(mode.parse::<f32>()?)
    };
    if quality.is_some_and(|q| !q.is_finite() || !(0. ..=100.).contains(&q)) {
        return Err("expected quality in 0..=100".into());
    }
    let config = match quality {
        None => ImiEncoderConfig::lossless(),
        Some(q) => ImiEncoderConfig::lossy(LossyQuality::JpegQuality(q))
            .with_decode_speed(DecodeSpeed::Fast)
            .tune_lossy(|t| t.adaptive_partitions = false),
    }
    .with_effort(10.)
    .with_threads(1)
    .with_checksum(true);
    let catalog = PyramidCatalog::new(vec![PyramidLevel::new(
        0,
        width,
        height,
        WIDTH,
        HEIGHT,
        TileOrder::RowMajor,
        1,
    )?])?
    .encode();
    let mut src = Reader::open(SeekReader::new(BufReader::new(File::open(source)?)))?;
    let mut entries = src.listing()?;
    let first = entries.next(&mut src)?.ok_or("empty source archive")?;
    let first_key = if first.key() == 0
        && src
            .read_entry_bounded(&first, MAX_TILE)?
            .starts_with(imij_set::CATALOG_MAGIC)
    {
        1
    } else {
        first.key()
    };
    let mut entries = src.listing_from(first_key)?;
    let mut dst =
        Writer::new(BufWriter::new(fresh(path)?)).with_index_placement(IndexPlacement::AtCommit)?;
    dst.insert(
        0,
        catalog.len() as u64,
        0,
        Summary::default(),
        &mut catalog.as_slice(),
    )?;
    for start in (0..count).step_by(32) {
        let input: Vec<_> = (start..count.min(start + 32))
            .map(|i| -> Result<_> {
                let entry = entries.next(&mut src)?.ok_or("missing source tile")?;
                if entry.key() != first_key + u64::from(i) {
                    return Err("unexpected source tile key".into());
                }
                let w = (width - i % cols * WIDTH).min(WIDTH);
                let h = (height - i / cols * HEIGHT).min(HEIGHT);
                Ok((i, w, h, src.read_entry_bounded(&entry, MAX_TILE)?))
            })
            .collect::<Result<_>>()?;
        let output = input
            .into_par_iter()
            .map(|(i, w, h, bytes)| {
                let header = imij::header(&bytes)?;
                if header.coding != imij::Coding::Lossless {
                    return Err(imij::Error::InvalidInput("source tile must be lossless"));
                }
                let (info, pixels) = imij::decode(&bytes)?;
                if (info.width, info.height, info.channels, info.sample)
                    != (w, h, Channels::Rgb, SampleType::U8)
                {
                    return Err(imij::Error::InvalidInput("source RGB8 tile geometry"));
                }
                let encoded = config.encode_samples(
                    ImageRef::new(&pixels, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )?;
                let encoded = imij::add_pixel_checksum(encoded)?;
                let (reconstructed, bytes) = imij::decode(&encoded)?;
                if imij::header(&encoded)?.version != 4
                    || reconstructed != info
                    || (quality.is_none() && pixels != bytes)
                {
                    return Err(imij::Error::InvalidInput("V4 tile reconstruction"));
                }
                Ok((i, w, h, encoded))
            })
            .collect::<std::result::Result<Vec<_>, imij::Error>>()?;
        for (i, w, h, encoded) in output {
            let mut summary = Summary::default();
            summary.width = w;
            summary.height = h;
            dst.insert(
                u64::from(i) + 1,
                encoded.len() as u64,
                0,
                summary,
                &mut encoded.as_slice(),
            )?;
        }
        if start.is_multiple_of(2048) {
            eprintln!("{} of {count} tiles", count.min(start + 32));
        }
    }
    if entries.next(&mut src)?.is_some() {
        return Err("extra source tiles".into());
    }
    dst.finish()?;
    offsets(path, &path.with_extension("index"))?;
    eprintln!("verified {count} V4 tiles");
    Ok(())
}
