//! Bounded preparation of supplied half-step imagery pyramids and lossy tile archives.
//! No new codec format: lossy cells are ordinary IMI images in imij-set.
#[path = "nasa_pyramid/native.rs"]
mod native;

use imij::planes::{Rect, SeekReader};
use imij::v4::{NumericFileEncoder, ObjectIntegrity, SpatialConfig, TileLimits, TileOrder};
use imij::{Channels, DecodeSpeed, ImageInfo, LossyQuality, Metadata, SampleType};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use zenresize::{Filter, PixelDescriptor, ResizeConfig, StreamingResize};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const TW: u32 = 512;
const TH: u32 = 256;
fn fresh(p: impl AsRef<Path>) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(p)
}
struct Raster {
    width: u32,
    height: u32,
    side: u32,
    files: Vec<File>,
}
impl Raster {
    fn band(&mut self, y: u32, rows: u32) -> Result<Vec<u8>> {
        let stride = self.width as usize * 3;
        let mut out = vec![0; stride * rows as usize];
        for row in 0..rows {
            let yy = y + row;
            if self.files.len() == 1 {
                self.files[0].seek(SeekFrom::Start(u64::from(yy) * stride as u64))?;
                self.files[0]
                    .read_exact(&mut out[row as usize * stride..(row as usize + 1) * stride])?;
            } else {
                for col in 0..2 {
                    let file = &mut self.files[(yy / self.side * 2 + col) as usize];
                    file.seek(SeekFrom::Start(
                        u64::from(yy % self.side) * u64::from(self.side) * 3,
                    ))?;
                    let at = row as usize * stride + col as usize * self.side as usize * 3;
                    file.read_exact(&mut out[at..at + self.side as usize * 3])?;
                }
            }
        }
        Ok(out)
    }
}
fn info(w: u32, h: u32) -> ImageInfo {
    ImageInfo::new(w, h, Channels::Rgb, SampleType::U8)
}
fn quick(raw: &Path, w: u32, h: u32, out: &Path) -> Result<()> {
    let count = u64::from(w.div_ceil(TW)) * u64::from(h.div_ceil(TH));
    let mut encoder = NumericFileEncoder::new(
        info(w, h),
        &Metadata::default(),
        &imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(TW, TH, 16 << 20)))
            .with_quick(imij::QuickBitSmarts::Off),
        SpatialConfig {
            tile_columns: 2,
            tile_rows: 9,
            order: TileOrder::Morton,
            integrity: ObjectIntegrity::ObjectsAndTiles,
            limits: TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        },
        BufWriter::new(fresh(out)?),
        fresh(out.with_extension("index"))?,
        count
            .checked_mul(128)
            .and_then(|n| n.checked_add(4096))
            .ok_or("index extent")?,
        u64::MAX,
    )?;
    let mut file = File::open(raw)?;
    for _ in 0..encoder.layout().tile_count() {
        encoder.begin_tile()?;
        while let Some(g) = encoder.next_group() {
            let stride = g.width as usize * 3;
            let mut bytes = vec![0; stride * g.height as usize];
            for yy in 0..g.height {
                file.seek(SeekFrom::Start(
                    (u64::from(g.y + yy) * u64::from(w) + u64::from(g.x)) * 3,
                ))?;
                file.read_exact(&mut bytes[yy as usize * stride..(yy as usize + 1) * stride])?;
            }
            encoder.push_group(&bytes, stride)?;
        }
        encoder.end_tile()?;
    }
    let (mut sink, _) = encoder.finish()?;
    sink.flush()?;
    Ok(())
}
struct ImageTiles {
    writer: imij_set::Writer<BufWriter<File>>,
    hashes: Vec<[u8; 32]>,
    width: u32,
    height: u32,
    quality: f32,
    lossless: bool,
}
impl ImageTiles {
    fn new(path: &Path, w: u32, h: u32) -> Result<Self> {
        Self::with_quality(path, w, h, 80.0)
    }
    fn with_quality(path: &Path, w: u32, h: u32, quality: f32) -> Result<Self> {
        if !quality.is_finite() || !(0.0..=100.0).contains(&quality) || w == 0 || h == 0 {
            return Err("lossy dimensions or quality".into());
        }
        // This pyramid profile reserves key zero for its versioned catalog.
        // Generic archives retain their arbitrary numeric-key contract.
        let catalog = imij_set::PyramidCatalog::new(vec![imij_set::PyramidLevel::new(
            0,
            w,
            h,
            TW,
            TH,
            imij_set::TileOrder::RowMajor,
            1,
        )?])?
        .encode();
        let mut writer = imij_set::Writer::new(BufWriter::new(fresh(path)?));
        writer.insert(
            imij_set::CATALOG_KEY,
            u64::try_from(catalog.len())?,
            0,
            imij_set::Summary::default(),
            &mut catalog.as_slice(),
        )?;
        Ok(Self {
            writer,
            hashes: Vec::new(),
            width: w,
            height: h,
            quality,
            lossless: false,
        })
    }
    fn band(&mut self, band: &[u8], rows: u32) -> Result<()> {
        let w = self.width;
        let cfg = imij::ImiEncoderConfig::lossy(90.0)
            .with_quality(LossyQuality::JpegQuality(self.quality))
            .with_effort(10.0)
            .with_decode_speed(DecodeSpeed::Fast)
            .with_threads(1);
        let cells: Vec<_> = (0..w.div_ceil(TW))
            .into_par_iter()
            .map(|col| {
                let x = col * TW;
                let cw = (w - x).min(TW);
                let stride = cw as usize * 3;
                let mut raw = vec![0; stride * rows as usize];
                for yy in 0..rows as usize {
                    raw[yy * stride..(yy + 1) * stride].copy_from_slice(
                        &band[yy * w as usize * 3 + x as usize * 3
                            ..yy * w as usize * 3 + (x + cw) as usize * 3],
                    );
                }
                let encoded = if self.lossless {
                    {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.with_effort(10.);
                        cfg = cfg.with_threads(1);
                        cfg
                    }
                    .encode_samples(
                        imij::ImageRef::new(&raw, stride, &info(cw, rows)),
                        &Metadata::default(),
                    )?
                } else {
                    cfg.encode_samples(
                        imij::ImageRef::new(&raw, stride, &info(cw, rows)),
                        &Metadata::default(),
                    )?
                };
                let (decoded_info, decoded) = imij::decode(&encoded)?;
                if self.lossless && decoded != raw {
                    return Err(imij::Error::InvalidInput("lossless tile pixels"));
                }
                if decoded_info != info(cw, rows) {
                    return Err(imij::Error::InvalidInput("tile geometry"));
                }
                Ok((encoded, Sha256::digest(&decoded).into(), cw))
            })
            .collect::<std::result::Result<_, imij::Error>>()?;
        for (encoded, hash, cw) in cells {
            let mut summary = imij_set::Summary::default();
            summary.width = cw;
            summary.height = rows;
            self.writer.insert(
                u64::try_from(self.hashes.len())?
                    .checked_add(1)
                    .ok_or("tile key overflow")?,
                encoded.len() as u64,
                0,
                summary,
                &mut encoded.as_slice(),
            )?;
            self.hashes.push(hash);
        }
        Ok(())
    }
    fn finish(self, path: &Path) -> Result<()> {
        let (mut sink, _) = self.writer.finish()?;
        sink.flush()?;
        drop(sink);
        let mut reader = imij_set::Reader::open(SeekReader::new(File::open(path)?))?;
        let mut listing = reader.listing()?;
        let mut index = BufWriter::new(fresh(path.with_extension("bin"))?);
        index.write_all(b"IMITILE1")?;
        for value in [self.width, self.height, TW, TH, self.hashes.len() as u32] {
            index.write_all(&value.to_le_bytes())?;
        }
        let mut count = 0;
        while let Some(entry) = listing.next(&mut reader)? {
            if entry.key() == imij_set::CATALOG_KEY {
                continue;
            }
            if entry.key()
                != u64::try_from(count)?
                    .checked_add(1)
                    .ok_or("tile key overflow")?
            {
                return Err("archive order".into());
            }
            index.write_all(&entry.offset().to_le_bytes())?;
            index.write_all(&u32::try_from(entry.len())?.to_le_bytes())?;
            index.write_all(&self.hashes[count])?;
            count += 1;
        }
        if count != self.hashes.len() {
            return Err("archive entry count".into());
        }
        index.flush()?;
        Ok(())
    }
}
struct RecordingSource {
    file: File,
    spans: Vec<(u64, Vec<u8>)>,
}
impl imij::planes::ReadAt for RecordingSource {
    fn len(&mut self) -> std::result::Result<u64, imij::Error> {
        self.file
            .metadata()
            .map(|m| m.len())
            .map_err(|_| imij::Error::Read)
    }
    fn read_exact_at(
        &mut self,
        offset: u64,
        out: &mut [u8],
    ) -> std::result::Result<(), imij::Error> {
        self.file
            .seek(SeekFrom::Start(offset))
            .and_then(|_| self.file.read_exact(out))
            .map_err(|_| imij::Error::Read)?;
        self.spans.push((offset, out.to_vec()));
        Ok(())
    }
}
fn bootstrap(input: &str, output: &str) -> Result<()> {
    let mut file = File::open(input)?;
    let length = file.metadata()?.len();
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let hash = hash.finalize();
    let reader = imij::region::RegionReader::open(RecordingSource {
        file,
        spans: Vec::new(),
    })?;
    if !reader.has_unit_table() {
        return Err("checked unit index required".into());
    }
    let recorded = reader.into_inner();
    let mut out = BufWriter::new(fresh(output)?);
    out.write_all(b"IMIRNG01")?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(&hash)?;
    out.write_all(&u32::try_from(recorded.spans.len())?.to_le_bytes())?;
    for (offset, bytes) in recorded.spans {
        out.write_all(&offset.to_le_bytes())?;
        out.write_all(&u32::try_from(bytes.len())?.to_le_bytes())?;
        out.write_all(&bytes)?;
    }
    out.flush()?;
    Ok(())
}
// Every level is resized from the original, avoiding accumulated resampling.
fn levels(mut raster: Raster, out: &Path, rasters_only: bool) -> Result<()> {
    std::fs::create_dir(out)?;
    let mut divisor = 2u32;
    let mut records = Vec::new();
    loop {
        let w = raster.width.div_ceil(divisor);
        let h = raster.height.div_ceil(divisor);
        let directory = out.join(format!("level-{divisor}"));
        std::fs::create_dir(&directory)?;
        let rawpath = directory.join("source.rgb");
        let mut raw = BufWriter::new(fresh(&rawpath)?);
        let config = ResizeConfig::builder(raster.width, raster.height, w, h)
            .filter(Filter::Lanczos)
            .format(PixelDescriptor::RGB8_SRGB)
            .linear()
            .build();
        let mut resize = StreamingResize::new(&config);
        for y in (0..raster.height).step_by(TH as usize) {
            let band = raster.band(y, (raster.height - y).min(TH))?;
            for row in band.chunks_exact(raster.width as usize * 3) {
                resize.push_row(row)?;
                while let Some(row) = resize.next_output_row() {
                    raw.write_all(row)?;
                }
            }
        }
        let n = resize.finish();
        for _ in 0..n {
            raw.write_all(resize.next_output_row().ok_or("resize drain")?)?;
        }
        raw.flush()?;
        drop(raw);
        if std::fs::metadata(&rawpath)?.len() != u64::from(w) * u64::from(h) * 3 {
            return Err("level raster extent".into());
        }
        if rasters_only {
            records.push(format!(
                "{{\"divisor\":{divisor},\"width\":{w},\"height\":{h}}}"
            ));
            eprintln!("prepared level 1/{divisor}: {w}x{h} (raster only)");
            if w <= TW && h <= TW {
                break;
            }
            divisor = divisor.checked_mul(2).ok_or("level divisor overflow")?;
            continue;
        }
        let lossless = directory.join("lossless.imi");
        quick(&rawpath, w, h, &lossless)?;
        bootstrap(
            lossless.to_str().ok_or("image path")?,
            directory
                .join("lossless-cache.bin")
                .to_str()
                .ok_or("cache path")?,
        )?;
        let lossy = directory.join("lossy.imijset");
        let mut tiles = ImageTiles::new(&lossy, w, h)?;
        let mut level = Raster {
            width: w,
            height: h,
            side: w,
            files: vec![File::open(&rawpath)?],
        };
        for y in (0..h).step_by(TH as usize) {
            let rows = (h - y).min(TH);
            tiles.band(&level.band(y, rows)?, rows)?;
        }
        tiles.finish(&lossy)?;
        // Compare a checked native ROI with the actual resized raster.
        let mut reader = imij::region::RegionReader::open(SeekReader::new(File::open(&lossless)?))?;
        let rw = w.min(32);
        let rh = h.min(32);
        let mut decoded = vec![0; rw as usize * rh as usize * 3];
        reader.decode_region(
            Rect::new(w - rw, h - rh, rw, rh),
            &mut decoded,
            rw as usize * 3,
        )?;
        let mut file = File::open(&rawpath)?;
        for y in 0..rh {
            let mut row = vec![0; rw as usize * 3];
            file.seek(SeekFrom::Start(
                (u64::from(h - rh + y) * u64::from(w) + u64::from(w - rw)) * 3,
            ))?;
            file.read_exact(&mut row)?;
            if row != decoded[y as usize * row.len()..(y as usize + 1) * row.len()] {
                return Err("level ROI mismatch".into());
            }
        }
        records.push(format!(
            "{{\"divisor\":{divisor},\"width\":{w},\"height\":{h}}}"
        ));
        eprintln!("prepared level 1/{divisor}: {w}x{h}");
        if w <= TW && h <= TW {
            break;
        }
        divisor = divisor.checked_mul(2).ok_or("level divisor overflow")?;
    }
    fresh(out.join("levels.json"))?.write_all(format!("[{}]\n", records.join(",")).as_bytes())?;
    Ok(())
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().collect();
    if a.get(1).is_some_and(|s| s == "native-tiles") {
        return native::pack_tiles(&a[2..]);
    }
    if a.get(1).is_some_and(|s| s == "native-raw") {
        return native::pack(&a[2..]);
    }
    if a.get(1).is_some_and(|s| s == "bootstrap") && a.len() == 4 {
        return bootstrap(&a[2], &a[3]);
    }
    if a.get(1)
        .is_some_and(|s| s == "lossless-raw" || s == "lossless-mosaic")
    {
        let (mut raster, output) = match a[1].as_str() {
            "lossless-raw" if a.len() == 6 => {
                let width: u32 = a[2].parse()?;
                let height: u32 = a[3].parse()?;
                if std::fs::metadata(&a[4])?.len() != u64::from(width) * u64::from(height) * 3 {
                    return Err("source raster extent".into());
                }
                (
                    Raster {
                        width,
                        height,
                        side: width,
                        files: vec![File::open(&a[4])?],
                    },
                    Path::new(&a[5]),
                )
            }
            "lossless-mosaic" if a.len() == 8 => {
                let side: u32 = a[2].parse()?;
                let width = side.checked_mul(2).ok_or("mosaic dimensions")?;
                let files = a[3..7]
                    .iter()
                    .map(File::open)
                    .collect::<std::io::Result<Vec<_>>>()?;
                for file in &files {
                    if file.metadata()?.len() != u64::from(side) * u64::from(side) * 3 {
                        return Err("mosaic raster extent".into());
                    }
                }
                (
                    Raster {
                        width,
                        height: width,
                        side,
                        files,
                    },
                    Path::new(&a[7]),
                )
            }
            _ => {
                return Err(
                    "lossless-raw W H RAW OUTPUT | lossless-mosaic SIDE TL TR BL BR OUTPUT".into(),
                );
            }
        };
        let mut tiles = ImageTiles::with_quality(output, raster.width, raster.height, 80.)?;
        tiles.lossless = true;
        for y in (0..raster.height).step_by(TH as usize) {
            let rows = (raster.height - y).min(TH);
            tiles.band(&raster.band(y, rows)?, rows)?;
            if y % (TH * 16) == 0 {
                eprintln!("prepared lossless effort10 rows {y}/{}", raster.height);
            }
        }
        return tiles.finish(output);
    }
    // Additional quality from the original RGB8 inputs, without transcoding existing lossy cells.
    if a.get(1)
        .is_some_and(|s| s == "lossy-raw" || s == "lossy-mosaic")
    {
        let (mut raster, output, quality) = match a[1].as_str() {
            "lossy-raw" if a.len() == 7 => {
                let w: u32 = a[2].parse()?;
                let h: u32 = a[3].parse()?;
                if std::fs::metadata(&a[4])?.len() != u64::from(w) * u64::from(h) * 3 {
                    return Err("source raster extent".into());
                }
                (
                    Raster {
                        width: w,
                        height: h,
                        side: w,
                        files: vec![File::open(&a[4])?],
                    },
                    Path::new(&a[5]),
                    a[6].parse()?,
                )
            }
            "lossy-mosaic" if a.len() == 9 => {
                let side: u32 = a[2].parse()?;
                let width = side.checked_mul(2).ok_or("mosaic dimensions")?;
                let files = a[3..7]
                    .iter()
                    .map(File::open)
                    .collect::<std::io::Result<Vec<_>>>()?;
                for file in &files {
                    if file.metadata()?.len() != u64::from(side) * u64::from(side) * 3 {
                        return Err("mosaic raster extent".into());
                    }
                }
                (
                    Raster {
                        width,
                        height: width,
                        side,
                        files,
                    },
                    Path::new(&a[7]),
                    a[8].parse()?,
                )
            }
            _ => return Err(
                "lossy-raw W H RAW OUTPUT QUALITY | lossy-mosaic SIDE TL TR BL BR OUTPUT QUALITY"
                    .into(),
            ),
        };
        let mut tiles = ImageTiles::with_quality(output, raster.width, raster.height, quality)?;
        for y in (0..raster.height).step_by(TH as usize) {
            let rows = (raster.height - y).min(TH);
            tiles.band(&raster.band(y, rows)?, rows)?;
            if y % (TH * 16) == 0 {
                eprintln!("prepared quality {quality} rows {y}/{}", raster.height);
            }
        }
        return tiles.finish(output);
    }
    // Full image plus consecutive half-step levels, without redundant quarter output.
    if a.get(1).is_some_and(|s| s == "complete-raw") && a.len() == 6 {
        let w = a[2].parse()?;
        let h = a[3].parse()?;
        let raw = Path::new(&a[4]);
        if std::fs::metadata(raw)?.len() != u64::from(w) * u64::from(h) * 3 {
            return Err("source raster extent".into());
        }
        let out = Path::new(&a[5]);
        std::fs::create_dir(out)?;
        let mut raster = Raster {
            width: w,
            height: h,
            side: w,
            files: vec![File::open(raw)?],
        };
        let lossy = out.join("full-lossy.imijset");
        let mut tiles = ImageTiles::new(&lossy, w, h)?;
        for y in (0..h).step_by(TH as usize) {
            let rows = (h - y).min(TH);
            tiles.band(&raster.band(y, rows)?, rows)?;
            if y % (TH * 16) == 0 {
                eprintln!("prepared full rows {y}/{h}");
            }
        }
        tiles.finish(&lossy)?;
        quick(raw, w, h, &out.join("full-lossless.imi"))?;
        bootstrap(
            out.join("full-lossless.imi").to_str().ok_or("image path")?,
            out.join("full-lossless-cache.bin")
                .to_str()
                .ok_or("cache path")?,
        )?;
        return levels(raster, &out.join("pyramid"), false);
    }
    // `rasters-raw` resizes every level straight from the original and writes only the
    // level RGB8 rasters (for the effort-10 encoders), not the Quick/Q80 byproducts.
    let rasters_only = a.get(1).is_some_and(|s| s == "rasters-raw");
    let pyramid = rasters_only || a.get(1).is_some_and(|s| s.starts_with("levels-"));
    let mode = a.get(1).map(|s| {
        s.strip_prefix("levels-")
            .or(s.strip_prefix("rasters-"))
            .unwrap_or(s)
    });
    let (mut raster,out)=match mode {
        Some("mosaic") if a.len()==8=> {let side:u32=a[2].parse()?;(Raster{width:side*2,height:side*2,side,files:a[3..7].iter().map(File::open).collect::<std::io::Result<_>>()?},Path::new(&a[7]))},
        Some("raw") if a.len()==6=> {let w=a[2].parse()?;let h=a[3].parse()?;(Raster{width:w,height:h,side:w,files:vec![File::open(&a[4])?]},Path::new(&a[5]))},
        _=>return Err("nasa_pyramid mosaic SIDE TL.rgb TR.rgb BL.rgb BR.rgb FRESH_DIR | raw W H INPUT.rgb FRESH_DIR".into())
    };
    if pyramid {
        return levels(raster, out, rasters_only);
    }
    std::fs::create_dir(out)?;
    let w = raster.width;
    let h = raster.height;
    let qw = w.div_ceil(4);
    let qh = h.div_ceil(4);
    let config = ResizeConfig::builder(w, h, qw, qh)
        .filter(Filter::Lanczos)
        .format(PixelDescriptor::RGB8_SRGB)
        .linear()
        .build();
    let mut resize = StreamingResize::new(&config);
    let rawpath = out.join("quarter.rgb");
    let mut quarter = BufWriter::new(fresh(&rawpath)?);
    let fullpath = out.join("full-lossy.imijset");
    let mut full = ImageTiles::new(&fullpath, w, h)?;
    for y in (0..h).step_by(TH as usize) {
        let rows = (h - y).min(TH);
        let band = raster.band(y, rows)?;
        full.band(&band, rows)?;
        for row in band.chunks_exact(w as usize * 3) {
            resize.push_row(row)?;
            while let Some(row) = resize.next_output_row() {
                quarter.write_all(row)?;
            }
        }
        if y % (TH * 8) == 0 {
            eprintln!("prepared full rows {y}/{h}");
        }
    }
    let n = resize.finish();
    for _ in 0..n {
        quarter.write_all(resize.next_output_row().ok_or("resize drain")?)?;
    }
    quarter.flush()?;
    drop(quarter);
    full.finish(&fullpath)?;
    if std::fs::metadata(&rawpath)?.len() != u64::from(qw) * u64::from(qh) * 3 {
        return Err("quarter raster extent".into());
    }
    quick(&rawpath, qw, qh, &out.join("quarter-lossless.imi"))?;
    let qpath = out.join("quarter-lossy.imijset");
    let mut loss = ImageTiles::new(&qpath, qw, qh)?;
    let mut q = Raster {
        width: qw,
        height: qh,
        side: qw,
        files: vec![File::open(&rawpath)?],
    };
    for y in (0..qh).step_by(TH as usize) {
        let rows = (qh - y).min(TH);
        loss.band(&q.band(y, rows)?, rows)?;
    }
    loss.finish(&qpath)?;
    if raster.files.len() == 1 {
        quick(Path::new(&a[4]), w, h, &out.join("full-lossless.imi"))?;
    }
    println!(
        "prepared {w}x{h}; quarter {qw}x{qh}; lossy JPEG-quality80 Fast effort10; linear-light Lanczos"
    );
    // One native indexed-region check of the supplied quarter raster.
    let mut reader = imij::region::RegionReader::open(SeekReader::new(File::open(
        out.join("quarter-lossless.imi"),
    )?))?;
    let rw = qw.min(32);
    let rh = qh.min(32);
    let mut decoded = vec![0; rw as usize * rh as usize * 3];
    reader.decode_region(Rect::new(0, 0, rw, rh), &mut decoded, rw as usize * 3)?;
    let mut f = File::open(&rawpath)?;
    for y in 0..rh {
        let mut row = vec![0; rw as usize * 3];
        f.seek(SeekFrom::Start(u64::from(y) * u64::from(qw) * 3))?;
        f.read_exact(&mut row)?;
        assert_eq!(
            row,
            decoded[y as usize * row.len()..(y as usize + 1) * row.len()]
        );
    }
    Ok(())
}
