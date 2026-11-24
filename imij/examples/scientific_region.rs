//! Measure indexed group encoding and local region reads of one raw F32 matrix.
//! encode RAW WIDTH HEIGHT GROUP_COLS GROUP_ROWS OUTPUT SCRATCH
//! probe RAW ENCODED WIDTH HEIGHT X Y REGION_WIDTH REGION_HEIGHT HOT_RUNS

use imij::planes::{Rect, SeekReader};
use imij::region::RegionReader;
use imij::stream_encode::IndexedGroupEncoder;
use imij::{Channels, Error, ImageInfo, Limits, Metadata, SampleType, decode_reader};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Instant;

fn arg<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T::Err: std::error::Error + 'static,
{
    Ok(args.next().ok_or("missing argument")?.parse()?)
}

fn source_region(
    source: &mut File,
    width: u32,
    rect: Rect,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let stride = rect.width() as usize * 4;
    let mut pixels = vec![0; stride * rect.height() as usize];
    for row in 0..rect.height() as usize {
        let at = ((rect.y() as usize + row) * width as usize + rect.x() as usize) * 4;
        source.seek(SeekFrom::Start(at as u64))?;
        source.read_exact(&mut pixels[row * stride..(row + 1) * stride])?;
    }
    Ok(pixels)
}

fn encode(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let raw = PathBuf::from(args.next().ok_or("missing raw path")?);
    let width: u32 = arg(&mut args)?;
    let height: u32 = arg(&mut args)?;
    let columns: u32 = arg(&mut args)?;
    let rows: u32 = arg(&mut args)?;
    let output = PathBuf::from(args.next().ok_or("missing output path")?);
    let scratch = PathBuf::from(args.next().ok_or("missing scratch path")?);
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    if columns == 0 || rows == 0 {
        return Err("group dimensions must be nonzero".into());
    }
    let expected = u64::from(width) * u64::from(height) * 4;
    if fs::metadata(&raw)?.len() != expected {
        return Err("raw F32 block size differs from the declared dimensions".into());
    }
    let info = ImageInfo::new(width, height, Channels::Gray, SampleType::F32);
    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let scratch_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&scratch)?;
    let group_count = u64::from(width.div_ceil(columns)) * u64::from(height.div_ceil(rows));
    let started = Instant::now();
    let mut writer = IndexedGroupEncoder::new(
        BufWriter::with_capacity(1 << 20, output_file),
        scratch_file,
        info.clone(),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            columns,
            rows,
            2 << 30,
        ))),
        group_count * 10,
    )?;
    let mut input = File::open(&raw)?;
    let mut groups = 0u64;
    while let Some(group) = writer.next_group() {
        let rect = Rect::new(group.x, group.y, group.width, group.height);
        let pixels = source_region(&mut input, width, rect)?;
        writer.write_group(&pixels, group.width as usize * 4)?;
        groups += 1;
    }
    let (buffered, _) = writer.finish()?;
    buffered.into_inner()?.sync_all()?;
    let encode_ns = started.elapsed().as_nanos();
    if groups != group_count {
        return Err("encoded group count differs".into());
    }
    let encoded = fs::read(&output)?;
    let source = fs::read(&raw)?;
    let mut decoded = vec![0u8; source.len()];
    let got_info = imij::decode_into_threads(&encoded, &mut decoded, width as usize * 4, 1)?;
    if got_info != info || decoded != source {
        return Err("indexed group image did not decode to original F32 bits".into());
    }
    println!(
        "mode=encode width={width} height={height} group_cols={columns} group_rows={rows} groups={groups} encoded_bytes={} scratch_bytes={} encode_ns={encode_ns} exact=true",
        encoded.len(),
        fs::metadata(&scratch)?.len(),
    );
    Ok(())
}

fn probe(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let raw = PathBuf::from(args.next().ok_or("missing raw path")?);
    let encoded = PathBuf::from(args.next().ok_or("missing encoded path")?);
    let width: u32 = arg(&mut args)?;
    let height: u32 = arg(&mut args)?;
    let x: u32 = arg(&mut args)?;
    let y: u32 = arg(&mut args)?;
    let region_width: u32 = arg(&mut args)?;
    let region_height: u32 = arg(&mut args)?;
    let hot_runs: usize = arg(&mut args)?;
    if args.next().is_some() || hot_runs == 0 {
        return Err("unexpected argument or zero hot runs".into());
    }
    let rect = Rect::new(x, y, region_width, region_height);
    let mut raw_file = File::open(raw)?;
    let expected = source_region(&mut raw_file, width, rect)?;
    let mut output = vec![0u8; expected.len()];
    let stride = region_width as usize * 4;
    let opened_at = Instant::now();
    let mut reader = RegionReader::open(SeekReader::new(File::open(&encoded)?))?;
    let open_ns = opened_at.elapsed().as_nanos();
    if reader.info() != &ImageInfo::new(width, height, Channels::Gray, SampleType::F32)
        || !reader.has_unit_table()
    {
        return Err("region reader has wrong dimensions or lacks index".into());
    }
    let open_stats = reader.read_stats();
    reader.reset_read_stats();
    let first_at = Instant::now();
    reader.decode_region(rect, &mut output, stride)?;
    let first_ns = first_at.elapsed().as_nanos();
    if output != expected {
        return Err("first region bits differ".into());
    }
    let first_stats = reader.read_stats();
    let mut hot_ns = Vec::with_capacity(hot_runs);
    let mut hot_bytes = Vec::with_capacity(hot_runs);
    for _ in 0..hot_runs {
        reader.reset_read_stats();
        let started = Instant::now();
        reader.decode_region(rect, &mut output, stride)?;
        hot_ns.push(started.elapsed().as_nanos());
        hot_bytes.push(reader.read_stats().bytes);
    }
    if output != expected {
        return Err("warmed region bits differ".into());
    }
    hot_ns.sort_unstable();
    println!(
        "mode=probe file_bytes={} width={width} height={height} x={x} y={y} region_width={region_width} region_height={region_height} open_ns={open_ns} first_decode_ns={first_ns} open_reads={} open_bytes={} first_reads={} first_bytes={} hot_median_ns={} hot_worst_ns={} hot_bytes={} hot_runs={hot_runs} exact=true",
        fs::metadata(encoded)?.len(),
        open_stats.reads,
        open_stats.bytes,
        first_stats.reads,
        first_stats.bytes,
        hot_ns[hot_runs / 2],
        hot_ns[hot_runs - 1],
        hot_bytes[0],
    );
    Ok(())
}

fn verify(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn std::error::Error>> {
    let raw = PathBuf::from(args.next().ok_or("missing raw path")?);
    let encoded = PathBuf::from(args.next().ok_or("missing encoded path")?);
    let width: u32 = arg(&mut args)?;
    let height: u32 = arg(&mut args)?;
    if args.next().is_some() {
        return Err("unexpected extra argument".into());
    }
    let want_info = ImageInfo::new(width, height, Channels::Gray, SampleType::F32);
    if fs::metadata(&raw)?.len() != u64::from(width) * u64::from(height) * 4 {
        return Err("raw F32 byte length differs from dimensions".into());
    }
    let mut raw_file = File::open(raw)?;
    let mut expected = Vec::new();
    let mut rows = 0u32;
    let started = Instant::now();
    let (got_info, _) = decode_reader(File::open(&encoded)?, &Limits::DEFAULT, |info, y, row| {
        if info != &want_info || y != rows {
            return Err(Error::Corrupt("unexpected F32 row declaration or ordinal"));
        }
        expected.resize(row.len(), 0);
        raw_file
            .read_exact(&mut expected)
            .map_err(|_| Error::Read)?;
        if row != expected {
            return Err(Error::Corrupt("F32 row differs from raw bits"));
        }
        rows += 1;
        Ok(())
    })?;
    if got_info != want_info || rows != height {
        return Err("decoded F32 image dimensions or row count differ".into());
    }
    let mut extra = [0];
    if raw_file.read(&mut extra)? != 0 {
        return Err("raw F32 source grew during verification".into());
    }
    println!(
        "mode=verify width={width} height={height} rows={rows} encoded_bytes={} verify_ns={} exact=true",
        fs::metadata(encoded)?.len(),
        started.elapsed().as_nanos()
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("encode") => encode(args),
        Some("probe") => probe(args),
        Some("verify") => verify(args),
        _ => Err("expected encode, probe or verify".into()),
    }
}
