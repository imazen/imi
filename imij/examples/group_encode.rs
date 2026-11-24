//! Incrementally generated input for bounded-writer memory qualification.
//! Build first, then measure the executable in a fresh process with time -v.
use imij::stream_encode::{
    ChecksummedGroupConfig, ChecksummedGroupEncoder, ForwardBandConfig, ForwardRowConfig,
    GroupEncoder, GroupRect, GrowingHeightGroupEncoder, IndexedGroupEncoder, StreamEncodeError,
    encode_forward_bands_indexed, encode_forward_rows_indexed,
};
use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::time::Instant;

struct Observe<W> {
    inner: W,
    start: Instant,
    first: Option<f64>,
    bytes: u64,
}
impl<W: Write> Write for Observe<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(bytes)?;
        if n > 0 && self.first.is_none() {
            self.first = Some(self.start.elapsed().as_secs_f64());
        }
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

enum Writer<W> {
    Basic(Box<GroupEncoder<W>>),
    Indexed(Box<IndexedGroupEncoder<W, std::fs::File>>),
    Checked(Box<ChecksummedGroupEncoder<W, std::fs::File, std::fs::File>>),
}
impl<W: Write> Writer<W> {
    fn next_group(&self) -> Option<GroupRect> {
        match self {
            Self::Basic(e) => e.next_group(),
            Self::Indexed(e) => e.next_group(),
            Self::Checked(e) => e.next_group(),
        }
    }
    fn write_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), StreamEncodeError> {
        match self {
            Self::Basic(e) => e.write_group(pixels, stride),
            Self::Indexed(e) => e.write_group(pixels, stride),
            Self::Checked(e) => e.write_group(pixels, stride),
        }
    }
    fn finish(self) -> Result<W, StreamEncodeError> {
        match self {
            Self::Basic(e) => (*e).finish(),
            Self::Indexed(e) => (*e).finish().map(|(w, _)| w),
            Self::Checked(e) => (*e).finish().map(|(w, _, _)| w),
        }
    }
}

fn generated_group(g: GroupRect) -> (Vec<u8>, usize) {
    let stride = g.width as usize * 3 + 7;
    let mut pixels = vec![0xA5; stride * g.height as usize];
    for y in 0..g.height as usize {
        for x in 0..g.width as usize {
            let (xx, yy) = (g.x + x as u32, g.y + y as u32);
            let i = y * stride + x * 3;
            pixels[i] = (xx & 255) as u8;
            pixels[i + 1] = (yy & 255) as u8;
            pixels[i + 2] = ((xx ^ yy) & 255) as u8;
        }
    }
    (pixels, stride)
}

/// Generates the same RGB8 pixels as `generated_group`, in physical row order.
/// No row or image buffer is held by the source.
struct GeneratedRows {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
}

impl Read for GeneratedRows {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.len() < 3 || self.y == self.height {
            return Ok(0);
        }
        let pixels = ((bytes.len() / 3) as u32).min(self.width - self.x) as usize;
        for (offset, pixel) in bytes[..pixels * 3]
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .enumerate()
        {
            let x = self.x + offset as u32;
            *pixel = [
                (x & 255) as u8,
                (self.y & 255) as u8,
                ((x ^ self.y) & 255) as u8,
            ];
        }
        self.x += pixels as u32;
        if self.x == self.width {
            self.x = 0;
            self.y += 1;
        }
        Ok(pixels * 3)
    }
}

fn forward(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let use_band = args[1] == "--forward-band";
    if args.len() != if use_band { 9 } else { 7 } {
        return Err("usage: group_encode --forward[-band] WIDTH HEIGHT GROUP_COLUMNS [GROUP_ROWS] FRESH_OUTPUT FRESH_INDEX_SCRATCH [FRESH_BAND_SCRATCH]".into());
    }
    let (width, height, columns) = (
        args[2].parse::<u32>()?,
        args[3].parse::<u32>()?,
        args[4].parse::<u32>()?,
    );
    let rows = if use_band { args[5].parse::<u32>()? } else { 1 };
    if width == 0 || height == 0 || columns == 0 || rows == 0 {
        return Err("dimensions and group columns must be nonzero".into());
    }
    let source_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(3))
        .ok_or("raw image size overflow")?;
    let groups = u64::from(width.div_ceil(columns)) * u64::from(height.div_ceil(rows));
    let max_index = groups.checked_mul(32).ok_or("index limit overflow")?;
    let source_cap = usize::try_from(
        u64::from(columns.min(width))
            .checked_mul(u64::from(rows.min(height)))
            .and_then(|n| n.checked_mul(3))
            .ok_or("source buffer limit overflow")?,
    )?;
    let output_at = if use_band { 6 } else { 5 };
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[output_at])?;
    let index = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&args[output_at + 1])?;
    let start = Instant::now();
    let sink = Observe {
        inner: BufWriter::with_capacity(65536, output),
        start,
        first: None,
        bytes: 0,
    };
    let mut source = GeneratedRows {
        width,
        height,
        x: 0,
        y: 0,
    };
    let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
    let groups_cfg = imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
        columns,
        rows,
        16 << 20,
    )));
    let (mut sink, index, band_bytes) = if use_band {
        let band = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&args[output_at + 2])?;
        let band_limit = u64::from(width) * u64::from(rows.min(height)) * 3;
        let (sink, index, band) = encode_forward_bands_indexed(
            &mut source,
            sink,
            index,
            band,
            info,
            &Metadata::default(),
            ForwardBandConfig::new(
                groups_cfg,
                u64::from(width) * 3,
                source_cap,
                source_bytes,
                band_limit,
                max_index,
            ),
        )?;
        (sink, index, Some(band.metadata()?.len()))
    } else {
        let (sink, index) = encode_forward_rows_indexed(
            &mut source,
            sink,
            index,
            info,
            &Metadata::default(),
            ForwardRowConfig::new(
                groups_cfg,
                u64::from(width) * 3,
                source_cap,
                source_bytes,
                max_index,
            ),
        )?;
        (sink, index, None)
    };
    sink.flush()?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "mode={} width={width} height={height} columns={columns} rows={rows} groups={groups} bytes={} index_scratch_bytes={} band_scratch_bytes={} seconds={elapsed:.9} first_write_seconds={:.9} encode_mpx_s={:.6}",
        if use_band { "forward-band" } else { "forward" },
        sink.bytes,
        index.metadata()?.len(),
        band_bytes.unwrap_or(0),
        sink.first.ok_or("no write")?,
        width as f64 * height as f64 / 1e6 / elapsed,
    );
    Ok(())
}

fn real(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let use_band = args[1] == "--real-band";
    if args.len() != if use_band { 10 } else { 9 } {
        return Err("usage: group_encode --real[-band] WIDTH HEIGHT COLUMNS ROWS RGB_FILE FRESH_OUTPUT FRESH_INDEX_SCRATCH [FRESH_BAND_SCRATCH]".into());
    }
    let (width, height, columns, rows) = (
        args[2].parse::<u32>()?,
        args[3].parse::<u32>()?,
        args[4].parse::<u32>()?,
        args[5].parse::<u32>()?,
    );
    if width == 0 || height == 0 || columns == 0 || rows == 0 {
        return Err("dimensions and group geometry must be nonzero".into());
    }
    let source_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(3))
        .ok_or("raw image size overflow")?;
    let mut source = std::fs::File::open(&args[6])?;
    if source.metadata()?.len() != source_bytes {
        return Err("RGB source length does not match dimensions".into());
    }
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[7])?;
    let index = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&args[8])?;
    let groups = u64::from(width.div_ceil(columns)) * u64::from(height.div_ceil(rows));
    let max_index = groups.checked_mul(10).ok_or("index limit overflow")?;
    let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
    let group_cfg = imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
        columns,
        rows,
        16 << 20,
    )));
    let start = Instant::now();
    let sink = Observe {
        inner: BufWriter::with_capacity(65536, output),
        start,
        first: None,
        bytes: 0,
    };
    let (mut sink, index, band_bytes) = if use_band {
        let band = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&args[9])?;
        let group_bytes = u64::from(columns.min(width)) * u64::from(rows.min(height)) * 3;
        let band_limit = u64::from(width) * u64::from(rows.min(height)) * 3;
        let (sink, index, band) = encode_forward_bands_indexed(
            &mut source,
            sink,
            index,
            band,
            info,
            &Metadata::default(),
            ForwardBandConfig::new(
                group_cfg,
                u64::from(width) * 3,
                usize::try_from(group_bytes)?,
                source_bytes,
                band_limit,
                max_index,
            ),
        )?;
        (sink, index, Some(band.metadata()?.len()))
    } else if rows == 1 {
        let (sink, index) = encode_forward_rows_indexed(
            &mut source,
            sink,
            index,
            info,
            &Metadata::default(),
            ForwardRowConfig::new(
                group_cfg,
                u64::from(width) * 3,
                columns.min(width) as usize * 3,
                source_bytes,
                max_index,
            ),
        )?;
        (sink, index, None)
    } else {
        let group_bytes = usize::try_from(
            u64::from(columns.min(width))
                .checked_mul(u64::from(rows.min(height)))
                .and_then(|n| n.checked_mul(3))
                .ok_or("group source size overflow")?,
        )?;
        if group_bytes > 16 << 20 {
            return Err("group source buffer exceeds probe limit".into());
        }
        let mut group = Vec::new();
        group.try_reserve_exact(group_bytes)?;
        group.resize(group_bytes, 0);
        let mut encoder = IndexedGroupEncoder::new(
            sink,
            index,
            info,
            &Metadata::default(),
            &group_cfg,
            max_index,
        )?;
        while let Some(rect) = encoder.next_group() {
            let row_bytes = rect.width as usize * 3;
            for local_y in 0..rect.height as usize {
                let position = (u64::from(rect.y) + local_y as u64) * u64::from(width) * 3
                    + u64::from(rect.x) * 3;
                source.seek(SeekFrom::Start(position))?;
                source.read_exact(&mut group[local_y * row_bytes..(local_y + 1) * row_bytes])?;
            }
            encoder.write_group(&group[..row_bytes * rect.height as usize], row_bytes)?;
        }
        let (sink, index) = encoder.finish()?;
        (sink, index, None)
    };
    sink.flush()?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "mode={} width={width} height={height} columns={columns} rows={rows} groups={groups} source_bytes={source_bytes} bytes={} index_scratch_bytes={} band_scratch_bytes={} seconds={elapsed:.9} encode_mpx_s={:.6}",
        if use_band { "real-band" } else { "real" },
        sink.bytes,
        index.metadata()?.len(),
        band_bytes.unwrap_or(0),
        width as f64 * height as f64 / 1e6 / elapsed,
    );
    Ok(())
}

fn verify_real(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 6 {
        return Err("usage: group_encode --verify-real WIDTH HEIGHT RGB_FILE IMIJ_FILE".into());
    }
    let (width, height) = (args[2].parse::<u32>()?, args[3].parse::<u32>()?);
    let expected_info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
    let mut source = std::fs::File::open(&args[4])?;
    let source_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(3))
        .ok_or("raw image size overflow")?;
    if source.metadata()?.len() != source_bytes {
        return Err("RGB source length does not match dimensions".into());
    }
    let mut expected_row = vec![0u8; usize::try_from(u64::from(width) * 3)?];
    let mut next_y = 0;
    let limits = {
        let mut limits = imij::Limits::DEFAULT;
        limits.max_pixels = u64::from(width) * u64::from(height);
        limits.max_bytes = source_bytes;
        limits.verify_checksum = true;
        limits
    };
    imij::decode_reader(
        std::io::BufReader::new(std::fs::File::open(&args[5])?),
        &limits,
        |info, y, row| {
            if info != &expected_info || y != next_y || row.len() != expected_row.len() {
                return Err(imij::Error::Corrupt("real-photo probe geometry"));
            }
            source
                .read_exact(&mut expected_row)
                .map_err(|_| imij::Error::Corrupt("real-photo source short"))?;
            if row != expected_row {
                return Err(imij::Error::Corrupt("real-photo pixel mismatch"));
            }
            next_y += 1;
            Ok(())
        },
    )?;
    if next_y != height {
        return Err("missing decoded rows".into());
    }
    println!("verified_real width={width} height={height} all_pixels_exact=true");
    Ok(())
}

fn growing(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 7 {
        return Err("usage: group_encode --growing WIDTH HEIGHT FRESH_OUTPUT FRESH_DATA_SCRATCH FRESH_INDEX_SCRATCH".into());
    }
    let (width, height) = (args[2].parse::<u32>()?, args[3].parse::<u32>()?);
    if width == 0 || height == 0 {
        return Err("dimensions must be nonzero".into());
    }
    let output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[4])?;
    let data = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&args[5])?;
    let index = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&args[6])?;
    let start = Instant::now();
    let sink = Observe {
        inner: BufWriter::with_capacity(65536, output),
        start,
        first: None,
        bytes: 0,
    };
    let cfg = imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
        256,
        16,
        16 << 20,
    )));
    let raw_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(3))
        .ok_or("raw image size overflow")?;
    let max_data = raw_bytes.checked_mul(4).ok_or("data limit overflow")?;
    let max_index = u64::from(width.div_ceil(256))
        .checked_mul(u64::from(height.div_ceil(16)))
        .and_then(|n| n.checked_mul(10))
        .ok_or("index limit overflow")?;
    let mut encoder = GrowingHeightGroupEncoder::new(
        sink,
        data,
        index,
        ImageInfo::new(width, 0, Channels::Rgb, SampleType::U8),
        &Metadata::default(),
        &cfg,
        max_data,
        max_index,
    )?;
    let mut first_group = None;
    let mut groups = 0u64;
    let stripe_count = height.div_ceil(16);
    for stripe in 0..stripe_count {
        if stripe == stripe_count - 1 && height % 16 != 0 {
            encoder.set_final_height(height)?;
        }
        for _ in 0..width.div_ceil(256) {
            let rect = encoder.next_group().ok_or("missing growing group")?;
            let (pixels, stride) = generated_group(rect);
            encoder.write_group(&pixels, stride)?;
            groups += 1;
            if first_group.is_none() {
                first_group = Some(start.elapsed().as_secs_f64());
            }
        }
    }
    if height % 16 == 0 {
        encoder.set_final_height(height)?;
    }
    let (sink, data, index) = encoder.finish()?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "mode=growing width={width} height={height} groups={groups} bytes={} data_scratch_bytes={} index_scratch_bytes={} seconds={elapsed:.9} first_write_seconds={:.9} first_group_seconds={:.9} encode_mpx_s={:.6}",
        sink.bytes,
        data.metadata()?.len(),
        index.metadata()?.len(),
        sink.first.ok_or("no write")?,
        first_group.ok_or("no group")?,
        width as f64 * height as f64 / 1e6 / elapsed
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--real") {
        return real(&args);
    }
    if args.get(1).is_some_and(|a| a == "--real-band") {
        return real(&args);
    }
    if args.get(1).is_some_and(|a| a == "--verify-real") {
        return verify_real(&args);
    }
    if args.get(1).is_some_and(|a| a == "--forward") {
        return forward(&args);
    }
    if args.get(1).is_some_and(|a| a == "--forward-band") {
        return forward(&args);
    }
    if args.get(1).is_some_and(|a| a == "--growing") {
        return growing(&args);
    }
    if args.get(1).is_some_and(|a| a == "--verify") {
        if args.len() != 5 {
            return Err("usage: group_encode --verify WIDTH HEIGHT INPUT".into());
        }
        let (width, height) = (args[2].parse::<u32>()?, args[3].parse::<u32>()?);
        let expected = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
        let limits = {
            let mut limits = imij::Limits::DEFAULT;
            limits.max_pixels = width as u64 * height as u64;
            limits.max_bytes = width as u64 * height as u64 * 3;
            limits.verify_checksum = true;
            limits
        };
        let file = std::io::BufReader::new(std::fs::File::open(&args[4])?);
        let mut next_row = 0;
        imij::decode_reader(file, &limits, |info, y, row| {
            if info != &expected || y != next_row || row.len() != width as usize * 3 {
                return Err(imij::Error::Corrupt("resource probe geometry"));
            }
            for (x, pixel) in row.as_chunks::<3>().0.iter().enumerate() {
                if *pixel
                    != [
                        (x & 255) as u8,
                        (y & 255) as u8,
                        ((x as u32 ^ y) & 255) as u8,
                    ]
                {
                    return Err(imij::Error::Corrupt("resource probe pixel mismatch"));
                }
            }
            next_row += 1;
            Ok(())
        })?;
        if next_row != height {
            return Err("missing rows".into());
        }
        println!("verified width={width} height={height} all_pixels_exact=true");
        return Ok(());
    }
    if !(4..=6).contains(&args.len()) {
        return Err("usage: group_encode WIDTH HEIGHT FRESH_OUTPUT [FRESH_INDEX_SCRATCH [FRESH_CHECKSUM_SCRATCH]]".into());
    }
    let (width, height) = (args[1].parse::<u32>()?, args[2].parse::<u32>()?);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])?;
    let start = Instant::now();
    let sink = Observe {
        inner: BufWriter::with_capacity(65536, file),
        start,
        first: None,
        bytes: 0,
    };
    let info = ImageInfo::new(width, height, Channels::Rgb, SampleType::U8);
    let config = imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(
        256,
        16,
        16 << 20,
    )));
    let mut enc = if let Some(path) = args.get(4) {
        let scratch = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        let max_index = u64::from(width.div_ceil(256)) * u64::from(height.div_ceil(16)) * 10;
        if let Some(path) = args.get(5) {
            let checksums = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)?;
            Writer::Checked(Box::new(ChecksummedGroupEncoder::new(
                sink,
                scratch,
                checksums,
                info,
                &Metadata::default(),
                ChecksummedGroupConfig::new(config, max_index, 16, 32 << 20),
            )?))
        } else {
            Writer::Indexed(Box::new(IndexedGroupEncoder::new(
                sink,
                scratch,
                info,
                &Metadata::default(),
                &config,
                max_index,
            )?))
        }
    } else {
        Writer::Basic(Box::new(GroupEncoder::new(
            sink,
            info,
            &Metadata::default(),
            &config,
        )?))
    };
    let mut first_group = None;
    let mut groups = 0u64;
    while let Some(g) = enc.next_group() {
        let (pixels, stride) = generated_group(g);
        enc.write_group(&pixels, stride)?;
        groups += 1;
        if first_group.is_none() {
            first_group = Some(start.elapsed().as_secs_f64());
        }
    }
    let sink = enc.finish()?;
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "width={width} height={height} groups={groups} bytes={} seconds={elapsed:.9} first_write_seconds={:.9} first_group_seconds={:.9} encode_mpx_s={:.6}",
        sink.bytes,
        sink.first.ok_or("no write")?,
        first_group.ok_or("no group")?,
        width as f64 * height as f64 / 1e6 / elapsed
    );
    Ok(())
}
