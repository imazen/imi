//! Rewrite a lossless primary image with additional right-hand columns.
//!
//! Usage: rewrite_width OLD.imi ADDED.raw ADDED_WIDTH GROUP_COLS GROUP_ROWS OUT.imi INDEX.tmp
//! `ADDED.raw` contains tightly packed rows in the old image's pixel format.
//! The source is decoded once to validate it and retain its metadata, then
//! fetched in bounded rectangles while the new file is encoded in group order.

use imij::planes::{Rect, SeekReader};
use imij::region::RegionReader;
use imij::stream_encode::IndexedGroupEncoder;
use imij::{ImageInfo, Limits, decode_reader};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Instant;

type Failure = Box<dyn std::error::Error>;

fn arg<T: std::str::FromStr>(args: &mut impl Iterator<Item = String>) -> Result<T, Failure>
where
    T::Err: std::error::Error + 'static,
{
    Ok(args.next().ok_or("missing argument")?.parse()?)
}

struct GroupSource<'a> {
    old: &'a mut RegionReader<SeekReader<File>>,
    added: &'a mut File,
    old_width: u32,
    added_width: u32,
    bytes_per_pixel: usize,
}

impl GroupSource<'_> {
    fn read_group(&mut self, rect: Rect, out: &mut [u8], stride: usize) -> Result<(), Failure> {
        let old_columns = self.old_width.saturating_sub(rect.x()).min(rect.width());
        if old_columns != 0 {
            self.old.decode_region(
                Rect::new(rect.x(), rect.y(), old_columns, rect.height()),
                out,
                stride,
            )?;
        }
        let added_columns = rect.width() - old_columns;
        if added_columns != 0 {
            let first_added = rect.x().max(self.old_width) - self.old_width;
            let row_bytes = usize::try_from(added_columns)?
                .checked_mul(self.bytes_per_pixel)
                .ok_or("added row length overflow")?;
            let destination_column = usize::try_from(old_columns)?
                .checked_mul(self.bytes_per_pixel)
                .ok_or("destination column overflow")?;
            for row in 0..rect.height() {
                let source_sample = u64::from(rect.y() + row)
                    .checked_mul(u64::from(self.added_width))
                    .and_then(|v| v.checked_add(u64::from(first_added)))
                    .ok_or("added source offset overflow")?;
                let source_offset = source_sample
                    .checked_mul(self.bytes_per_pixel as u64)
                    .ok_or("added source byte offset overflow")?;
                let destination_offset = usize::try_from(row)?
                    .checked_mul(stride)
                    .and_then(|v| v.checked_add(destination_column))
                    .ok_or("destination row offset overflow")?;
                self.added.seek(SeekFrom::Start(source_offset))?;
                self.added
                    .read_exact(&mut out[destination_offset..destination_offset + row_bytes])?;
            }
        }
        Ok(())
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), Failure> {
    let old_path = PathBuf::from(args.next().ok_or("missing old image path")?);
    let added_path = PathBuf::from(args.next().ok_or("missing added-column path")?);
    let added_width: u32 = arg(&mut args)?;
    let group_columns: u32 = arg(&mut args)?;
    let group_rows: u32 = arg(&mut args)?;
    let output_path = PathBuf::from(args.next().ok_or("missing output path")?);
    let index_path = PathBuf::from(args.next().ok_or("missing index scratch path")?);
    if args.next().is_some() || added_width == 0 || group_columns == 0 || group_rows == 0 {
        return Err("extra argument or zero dimension".into());
    }

    let scanned_at = Instant::now();
    let (old_info, metadata) =
        decode_reader(File::open(&old_path)?, &Limits::DEFAULT, |_, _, _| Ok(()))?;
    let scan_ns = scanned_at.elapsed().as_nanos();
    let mut old = RegionReader::open(SeekReader::new(File::open(&old_path)?))?;
    if !old.has_unit_table() || old.header().coding.is_lossy() || old.info() != &old_info {
        return Err("source must be one indexed lossless primary image".into());
    }
    let width = old_info
        .width
        .checked_add(added_width)
        .ok_or("new width exceeds u32")?;
    let mut info: ImageInfo = old_info;
    info.width = width;
    let height = info.height;
    let bytes_per_pixel = info.bytes_per_pixel();
    let expected_added = u64::from(added_width)
        .checked_mul(u64::from(info.height))
        .and_then(|v| v.checked_mul(bytes_per_pixel as u64))
        .ok_or("added source length overflow")?;
    if fs::metadata(&added_path)?.len() != expected_added {
        return Err("added source size differs from declared dimensions".into());
    }
    let group_bytes = usize::try_from(group_columns.min(width))?
        .checked_mul(usize::try_from(group_rows.min(info.height))?)
        .and_then(|v| v.checked_mul(bytes_per_pixel))
        .ok_or("group allocation overflow")?;
    const WORKING_LIMIT: usize = 32 << 20;
    if group_bytes > WORKING_LIMIT {
        return Err("group exceeds the 32 MiB pixel-buffer cap".into());
    }
    let group_count = u64::from(width.div_ceil(group_columns))
        .checked_mul(u64::from(info.height.div_ceil(group_rows)))
        .ok_or("group count overflow")?;
    let max_index_bytes = group_count.checked_mul(10).ok_or("index cap overflow")?;
    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)?;
    let index_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&index_path)?;
    let mut added = File::open(&added_path)?;
    let mut source = GroupSource {
        old: &mut old,
        added: &mut added,
        old_width: width - added_width,
        added_width,
        bytes_per_pixel,
    };
    let started = Instant::now();
    let mut writer = IndexedGroupEncoder::new(
        BufWriter::with_capacity(1 << 20, output_file),
        index_file,
        info,
        &metadata,
        &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
            group_columns,
            group_rows,
            WORKING_LIMIT,
        ))),
        max_index_bytes,
    )?;
    let mut pixels = Vec::new();
    let mut groups = 0u64;
    while let Some(group) = writer.next_group() {
        let stride = usize::try_from(group.width)?
            .checked_mul(bytes_per_pixel)
            .ok_or("group stride overflow")?;
        let used = stride
            .checked_mul(usize::try_from(group.height)?)
            .ok_or("group length overflow")?;
        if used > pixels.len() {
            pixels.try_reserve_exact(used - pixels.len())?;
            pixels.resize(used, 0);
        }
        let rect = Rect::new(group.x, group.y, group.width, group.height);
        source.read_group(rect, &mut pixels[..used], stride)?;
        writer.write_group(&pixels[..used], stride)?;
        groups += 1;
    }
    let (buffered, _) = writer.finish()?;
    buffered.into_inner()?.sync_all()?;
    if groups != group_count {
        return Err("written group count differs from geometry".into());
    }
    let encode_ns = started.elapsed().as_nanos();
    println!(
        "mode=rewrite-width width={width} height={} added_width={added_width} groups={groups} scan_ns={scan_ns} encode_ns={encode_ns} output_bytes={} index_bytes={}",
        height,
        fs::metadata(&output_path)?.len(),
        fs::metadata(&index_path)?.len(),
    );
    Ok(())
}

fn main() -> Result<(), Failure> {
    run(std::env::args().skip(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use imij::planes::SliceReader;
    use imij::stream_encode::IndexedGroupEncoder;
    use imij::{Channels, Metadata, SampleType};
    use std::io::Cursor;

    #[test]
    fn widening_keeps_every_sample_and_metadata() -> Result<(), Failure> {
        let root = std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join("tmp")))
            .ok_or("test needs CARGO_TARGET_TMPDIR or HOME")?;
        let dir = root.join(format!("imij-width-rewrite-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let old_path = dir.join("old.imi");
        let added_path = dir.join("added.raw");
        let output_path = dir.join("wide.imi");
        let index_path = dir.join("index.tmp");

        let info = ImageInfo::new(7, 5, Channels::Gray, SampleType::F32);
        let mut old_raw = Vec::new();
        let mut added_raw = Vec::new();
        let mut expected = Vec::new();
        for y in 0..5u32 {
            let mut old_row = Vec::new();
            let mut added_row = Vec::new();
            for x in 0..7u32 {
                old_row.extend_from_slice(&(y * 31 + x * 17).to_le_bytes());
            }
            for x in 0..3u32 {
                added_row.extend_from_slice(&(0x8000_0000 | y * 37 + x).to_le_bytes());
            }
            old_raw.extend_from_slice(&old_row);
            added_raw.extend_from_slice(&added_row);
            expected.extend_from_slice(&old_row);
            expected.extend_from_slice(&added_row);
        }
        let metadata = Metadata {
            orientation: 3,
            xmp: Some(b"<width-test/>".to_vec()),
            ..Metadata::default()
        };
        let mut source = IndexedGroupEncoder::new(
            Vec::new(),
            Cursor::new(Vec::new()),
            info.clone(),
            &metadata,
            &imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
                3,
                2,
                32 << 20,
            ))),
            1000,
        )?;
        while let Some(group) = source.next_group() {
            let offset = group.y as usize * info.row_bytes() + group.x as usize * 4;
            source.write_group(&old_raw[offset..], info.row_bytes())?;
        }
        let (old_file, _) = source.finish()?;
        fs::write(&old_path, old_file)?;
        fs::write(&added_path, added_raw)?;
        run([
            old_path.display().to_string(),
            added_path.display().to_string(),
            "3".into(),
            "4".into(),
            "2".into(),
            output_path.display().to_string(),
            index_path.display().to_string(),
        ]
        .into_iter())?;
        let output = fs::read(&output_path)?;
        let mut got = Vec::new();
        let (wide_info, wide_meta) =
            decode_reader(output.as_slice(), &Limits::DEFAULT, |_, _, row| {
                got.extend_from_slice(row);
                Ok(())
            })?;
        assert_eq!(wide_info.width, 10);
        assert_eq!(wide_info.height, 5);
        assert_eq!(wide_meta, metadata);
        assert_eq!(got, expected);
        let mut region = RegionReader::open(SliceReader::new(&output))?;
        let mut seam = vec![0; 4 * 3 * 4];
        region.decode_region(Rect::new(6, 1, 4, 3), &mut seam, 16)?;
        for row in 0..3 {
            assert_eq!(
                &seam[row * 16..(row + 1) * 16],
                &expected[(row + 1) * 40 + 24..(row + 1) * 40 + 40]
            );
        }
        Ok(())
    }
}
