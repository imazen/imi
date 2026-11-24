//! Append raw F32 rows to a canonical indexed group image in a new file.
//! OLD RAW_TAIL WIDTH OLD_HEIGHT ADDED_ROWS GROUP_COLS GROUP_ROWS OUTPUT SCRATCH

use imij::decode_resources::DecodePolicy;
use imij::stream_encode::AppendRowsEncoder;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom};
use std::time::Instant;

fn argument<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T::Err: std::error::Error + 'static,
{
    Ok(args.next().ok_or("missing argument")?.parse()?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let old = args.next().ok_or("missing old path")?;
    let raw_tail = args.next().ok_or("missing raw tail path")?;
    let width: u32 = argument(&mut args)?;
    let old_height: u32 = argument(&mut args)?;
    let added_rows: u32 = argument(&mut args)?;
    let columns: u32 = argument(&mut args)?;
    let rows: u32 = argument(&mut args)?;
    let output = args.next().ok_or("missing output path")?;
    let scratch = args.next().ok_or("missing scratch path")?;
    if args.next().is_some() || width == 0 || added_rows == 0 || columns == 0 || rows == 0 {
        return Err("invalid geometry or extra argument".into());
    }
    let old_bytes = fs::metadata(&old)?.len();
    let raw_bytes = u64::from(width) * u64::from(added_rows) * 4;
    if fs::metadata(&raw_tail)?.len() != raw_bytes {
        return Err("raw tail length differs from declared geometry".into());
    }
    let count =
        u64::from(width.div_ceil(columns)) * u64::from((old_height + added_rows).div_ceil(rows));
    let mut tail = File::open(raw_tail)?;
    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let scratch_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&scratch)?;
    let started = Instant::now();
    let mut writer = AppendRowsEncoder::new(
        File::open(old)?,
        BufWriter::with_capacity(1 << 20, output_file),
        scratch_file,
        added_rows,
        &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            columns,
            rows,
            2 << 30,
        ))),
        count * 10,
        DecodePolicy::default().with_max_input_bytes(old_bytes),
    )?;
    let mut groups = 0u64;
    while let Some(group) = writer.next_group() {
        let row_bytes = group.width as usize * 4;
        let mut pixels = vec![0; row_bytes * group.height as usize];
        for row in 0..group.height {
            let source_at =
                (u64::from(group.y - old_height + row) * u64::from(width) + u64::from(group.x)) * 4;
            tail.seek(SeekFrom::Start(source_at))?;
            let start = row as usize * row_bytes;
            tail.read_exact(&mut pixels[start..start + row_bytes])?;
        }
        writer.write_group(&pixels, row_bytes)?;
        groups += 1;
    }
    let (buffered, _, _) = writer.finish()?;
    buffered.into_inner()?.sync_all()?;
    println!(
        "mode=append_rows width={width} old_height={old_height} added_rows={added_rows} group_cols={columns} group_rows={rows} new_groups={groups} old_bytes={old_bytes} output_bytes={} scratch_bytes={} append_ns={}",
        fs::metadata(&output)?.len(),
        fs::metadata(&scratch)?.len(),
        started.elapsed().as_nanos(),
    );
    Ok(())
}
