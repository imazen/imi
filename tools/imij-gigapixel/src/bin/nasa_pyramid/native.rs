use super::{Result, TH, TW, fresh, info};
use imij::planes::{
    ImageDirectory, ImageView, ImageViewRole, LevelDescriptor, PlaneDescriptor, PlaneSetLimits,
    PlaneSetReader, PlaneSetWriter, ReadAt, Rect, SeekReader, ViewBacking, ViewRegistration,
};
use imij::{
    Channels, Coding, HashAlgorithm, Hasher, ImageRef, ImiEncoderConfig, Metadata, SampleType,
};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

struct Level {
    width: u32,
    height: u32,
    divisor: u32,
    path: PathBuf,
}

pub(super) fn pack(args: &[String]) -> Result<()> {
    let [width, height, full, reduced, output, rest @ ..] = args else {
        return Err("native-raw W H ORIGINAL.rgb RESIZED_DIR FRESH_DIR [QUALITY]".into());
    };
    let width: u32 = width.parse()?;
    let height: u32 = height.parse()?;
    let quality = quality_argument(rest)?;
    let levels = levels(width, height, Path::new(full), |divisor| {
        Path::new(reduced).join(format!("level-{divisor}/source.rgb"))
    })?;
    for level in &levels {
        let bytes = u64::from(level.width)
            .checked_mul(u64::from(level.height))
            .and_then(|samples| samples.checked_mul(3))
            .ok_or("raster extent overflow")?;
        if std::fs::metadata(&level.path)?.len() != bytes {
            return Err(format!("raster extent for {}", level.path.display()).into());
        }
    }
    let thumbnail = levels.last().unwrap();
    let config = quality.map_or_else(ImiEncoderConfig::lossless, ImiEncoderConfig::lossy);
    let primary = config.encode_samples(
        ImageRef::packed(
            &std::fs::read(&thumbnail.path)?,
            &info(thumbnail.width, thumbnail.height),
        ),
        &Metadata::default(),
    )?;
    write_pyramid(
        &levels,
        Path::new(output),
        quality,
        &primary,
        |level, write| {
            let mut tile = vec![0u8; (TW * TH * 3) as usize];
            let mut file = File::open(&level.path)?;
            for y in (0..level.height).step_by(TH as usize) {
                for x in (0..level.width).step_by(TW as usize) {
                    let (w, h) = ((level.width - x).min(TW), (level.height - y).min(TH));
                    let row_bytes = w as usize * 3;
                    let pixels = &mut tile[..row_bytes * h as usize];
                    for (r, row) in pixels.chunks_exact_mut(row_bytes).enumerate() {
                        let at =
                            ((u64::from(y) + r as u64) * u64::from(level.width) + u64::from(x)) * 3;
                        file.seek(SeekFrom::Start(at))?;
                        file.read_exact(row)?;
                    }
                    let encoded = config.encode_samples(
                        ImageRef::packed(pixels, &info(w, h)),
                        &Metadata::default(),
                    )?;
                    write(&encoded)?;
                }
            }
            Ok(())
        },
    )
}

pub(super) fn pack_tiles(args: &[String]) -> Result<()> {
    let [width, height, input, output, rest @ ..] = args else {
        return Err("native-tiles W H LEVEL_ARCHIVES FRESH_DIR [QUALITY]".into());
    };
    let quality = quality_argument(rest)?;
    let input = Path::new(input);
    let levels = levels(
        width.parse()?,
        height.parse()?,
        &input.join("level-1.imijset"),
        |divisor| input.join(format!("level-{divisor}.imijset")),
    )?;
    let smallest = levels.last().unwrap();
    let image = info(smallest.width, smallest.height);
    let stride = image.row_bytes();
    let mut pixels = vec![0; image.checked_buffer_len().ok_or("primary extent")?];
    let columns = smallest.width.div_ceil(TW);
    tiles(smallest, |ordinal, payload| {
        let x = ordinal % columns * TW;
        let y = ordinal / columns * TH;
        let expected = info((smallest.width - x).min(TW), (smallest.height - y).min(TH));
        let header = imij::header(payload)?;
        if header.info != expected
            || header.coding
                != if quality.is_some() {
                    Coding::Lossy
                } else {
                    Coding::Lossless
                }
        {
            return Err("smallest-level tile geometry or color declaration".into());
        }
        let mut limits = imij::Limits::DEFAULT;
        limits.max_pixels = u64::from(expected.width) * u64::from(expected.height);
        limits.max_bytes = expected.checked_buffer_len().ok_or("tile extent")? as u64;
        let (_, decoded) = imij::decode_with_limits(payload, &limits)?;
        for (row, src) in decoded.chunks_exact(expected.row_bytes()).enumerate() {
            let start = (y as usize + row) * stride + x as usize * 3;
            pixels[start..start + src.len()].copy_from_slice(src);
        }
        Ok(())
    })?;
    let primary = ImiEncoderConfig::lossless()
        .encode_samples(ImageRef::packed(&pixels, &image), &Metadata::default())?;
    let primary = imij::add_pixel_checksum(primary)?;
    write_pyramid(
        &levels,
        Path::new(output),
        quality,
        &primary,
        |level, write| tiles(level, |_, payload| write(payload)),
    )
}

fn tiles(level: &Level, mut visit: impl FnMut(u32, &[u8]) -> Result<()>) -> Result<()> {
    let mut archive = imij_set::Reader::open(SeekReader::new(File::open(&level.path)?))?;
    let head = archive.get(0)?.ok_or("missing Explorer archive catalog")?;
    let catalog = imij_set::PyramidCatalog::decode(&archive.read_entry_bounded(&head, 64 << 10)?)?;
    let [row] = catalog.levels() else {
        return Err("expected single-level archive".into());
    };
    if (row.width, row.height, row.tile_width, row.tile_height)
        != (level.width, level.height, TW, TH)
        || row.order != imij_set::TileOrder::RowMajor
        || row.key_base != 1
    {
        return Err("invalid Explorer archive geometry".into());
    }
    let mut list = archive.listing_from(1)?;
    for i in 0..u32::try_from(row.tile_count())? {
        let entry = list.next(&mut archive)?.ok_or("missing Explorer tile")?;
        if entry.key() != u64::from(i) + 1 {
            return Err("invalid Explorer tile key".into());
        }
        let payload = archive.read_entry_bounded(&entry, 16 << 20)?;
        if imij::header(&payload)?.version != 4 {
            return Err("native migration requires V4 tile archives".into());
        }
        visit(i, &payload)?;
    }
    if list.next(&mut archive)?.is_some() {
        return Err("extra Explorer tiles".into());
    }
    Ok(())
}

fn quality_argument(rest: &[String]) -> Result<Option<f32>> {
    match rest {
        [] => Ok(None),
        [q] => {
            let q: f32 = q.parse()?;
            if !q.is_finite() || !(0.0..=100.0).contains(&q) {
                return Err("native pyramid quality must be 0 through 100".into());
            }
            Ok(Some(q))
        }
        _ => Err("expected at most one quality".into()),
    }
}

fn levels(
    width: u32,
    height: u32,
    full: &Path,
    reduced: impl Fn(u32) -> PathBuf,
) -> Result<Vec<Level>> {
    if width == 0 || height == 0 || width.max(height) <= TW {
        return Err("native pyramid requires nonempty geometry above 512 pixels".into());
    }
    let mut levels = vec![Level {
        width,
        height,
        divisor: 1,
        path: full.to_owned(),
    }];
    let mut divisor = 2u32;
    loop {
        let (width, height) = (width.div_ceil(divisor), height.div_ceil(divisor));
        levels.push(Level {
            width,
            height,
            divisor,
            path: reduced(divisor),
        });
        if width <= TW && height <= TW {
            break;
        }
        divisor = divisor.checked_mul(2).ok_or("level divisor overflow")?;
    }
    Ok(levels)
}

fn write_pyramid(
    levels: &[Level],
    output: &Path,
    quality: Option<f32>,
    primary: &[u8],
    mut read_level: impl FnMut(&Level, &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()>,
) -> Result<()> {
    let (width, height) = (levels[0].width, levels[0].height);
    let thumbnail = levels.last().unwrap();
    let mut record_count = 0u64;
    for level in levels {
        if level.divisor == thumbnail.divisor {
            continue;
        }
        record_count = record_count
            .checked_add(u64::from(level.width.div_ceil(TW)) * u64::from(level.height.div_ceil(TH)))
            .ok_or("record count overflow")?;
    }
    if record_count > 1 << 20 {
        return Err("native pyramid exceeds the record limit".into());
    }
    std::fs::create_dir(output)?;
    let coding = if quality.is_some() {
        Coding::Lossy
    } else {
        Coding::Lossless
    };
    let output_path = output.join("pyramid.imi.partial");
    let mut writer = PlaneSetWriter::new(
        primary,
        HashWriter {
            output: BufWriter::new(fresh(&output_path)?),
            hash: Hasher::new(HashAlgorithm::Xxh3_128),
        },
        fresh(output.join("directory.scratch"))?,
        PlaneSetLimits::default()
            .with_plane_pixels(1 << 40)
            .with_file_bytes(1 << 46),
        match record_count {
            0..=256 => 64,
            257..=4096 => 256,
            _ => 1024,
        },
        record_count.checked_mul(10).ok_or("index scratch size")?,
    )?;
    let mut planes = Vec::with_capacity(3);
    for name in ["red", "green", "blue"] {
        planes.push(writer.add_plane(PlaneDescriptor::new(name, SampleType::U8))?);
    }
    let mut views = Vec::new();
    // Reuse the ordinary primary as the smallest view.
    for level in levels {
        let id = u64::from(level.divisor);
        let label = if id == 1 {
            "full".to_owned()
        } else {
            format!("level-{id}")
        };
        let backing = if level.divisor == thumbnail.divisor {
            ViewBacking::Primary
        } else {
            let ordinal = writer.declare_color_level(
                &planes,
                LevelDescriptor::new(level.width, level.height, TW, TH),
                Channels::Rgb,
                coding,
            )?[0];
            ViewBacking::Level {
                plane: planes[0],
                level: ordinal,
            }
        };
        let mut view = ImageView::new(
            id,
            &label,
            if id == 1 {
                ImageViewRole::Original
            } else {
                ImageViewRole::Reduced
            },
            backing,
        );
        if id != 1 {
            view = view.with_registration(ViewRegistration::resized(
                width,
                height,
                level.width,
                level.height,
            )?);
        }
        views.push(view);
    }
    writer.set_image_directory(ImageDirectory::new(1, views)?)?;
    for (ordinal, level) in levels[..levels.len() - 1].iter().enumerate() {
        let mut records = writer.record_range(planes[0], u32::try_from(ordinal)?)?;
        read_level(level, &mut |encoded| {
            writer.write_tile(records.next().ok_or("extra tiles")?, encoded)?;
            Ok(())
        })?;
        if records.next().is_some() {
            return Err("extra tile record".into());
        }
    }
    let (writer, scratch) = writer.finish()?;
    let bytes = writer.output.get_ref().metadata()?.len();
    let digest = format!("{:x}", writer.hash.finish());
    drop(writer.output);
    drop(scratch);
    std::fs::rename(&output_path, output.join("pyramid.imi"))?;
    std::fs::remove_file(output.join("directory.scratch"))?;
    let opening = opening_ranges(&output.join("pyramid.imi"), levels)?;
    let rows: Vec<_> = levels
        .iter()
        .map(|l| {
            format!(
                "{{\"id\":\"{}\",\"divisor\":{},\"width\":{},\"height\":{}}}",
                l.divisor, l.divisor, l.width, l.height
            )
        })
        .collect();
    let quality = quality.map_or_else(|| "null".to_owned(), |q| q.to_string());
    writeln!(
        fresh(output.join("pyramid.json"))?,
        "{{\"contentId\":\"{digest}\",\"encodedBytes\":{bytes},\"qualityTarget\":{quality},\"opening\":[{opening}],\"views\":[{}]}}",
        rows.join(",")
    )?;
    Ok(())
}

const MAX_OPEN_BYTES: u64 = 256 << 10;
const MAX_OPEN_RANGES: usize = 8;

fn opening_ranges(path: &Path, levels: &[Level]) -> Result<String> {
    let mut reader = PlaneSetReader::open(
        OpeningReads {
            input: SeekReader::new(File::open(path)?),
            ranges: Vec::new(),
        },
        PlaneSetLimits::default()
            .with_plane_pixels(1 << 40)
            .with_file_bytes(1 << 46),
    )?;
    let mut pages = Vec::new();
    for level in levels
        .iter()
        .rev()
        .take_while(|level| level.width.max(level.height) <= 4096)
    {
        let id = u64::from(level.divisor);
        reader.view_info(id)?;
        pages.extend(reader.view_region_pages(id, Rect::new(0, 0, level.width, level.height))?);
    }
    let required = merge_reads(reader.into_inner().ranges);
    let mut plan = required.clone();
    plan.extend(pages);
    let plan = merge_reads(plan);
    let fits = |ranges: &[Range<u64>]| {
        ranges.len() <= MAX_OPEN_RANGES
            && ranges.iter().map(|r| r.end - r.start).sum::<u64>() <= MAX_OPEN_BYTES
    };
    let ranges = if fits(&plan) {
        plan
    } else if fits(&required) {
        required
    } else {
        Vec::new()
    };
    Ok(ranges
        .iter()
        .map(|r| format!("[{},{}]", r.start, r.end))
        .collect::<Vec<_>>()
        .join(","))
}

fn merge_reads(mut ranges: Vec<Range<u64>>) -> Vec<Range<u64>> {
    ranges.sort_unstable_by_key(|r| r.start);
    let mut merged: Vec<Range<u64>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut().filter(|last| last.end >= range.start) {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

struct OpeningReads {
    input: SeekReader<File>,
    ranges: Vec<Range<u64>>,
}
impl ReadAt for OpeningReads {
    fn len(&mut self) -> std::result::Result<u64, imij::Error> {
        self.input.len()
    }
    fn read_exact_at(
        &mut self,
        offset: u64,
        out: &mut [u8],
    ) -> std::result::Result<(), imij::Error> {
        self.input.read_exact_at(offset, out)?;
        if !out.is_empty() {
            self.ranges.push(offset..offset + out.len() as u64);
        }
        Ok(())
    }
}

struct HashWriter {
    output: BufWriter<File>,
    hash: Hasher,
}
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.output.write(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}
