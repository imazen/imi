//! Bounded import of published gallery sources into raw RGB8 level-0 rasters.
//!
//! `jpeg SOURCE.jpg FRESH.rgb FRESH.json` streams a baseline JPEG through zenjpeg's
//! scanline reader (one MCU strip at a time, the compressed file held once) into a raw
//! RGB8 file and records source/raster SHA-256, dimensions and the colour handling.
//! An embedded sRGB IEC61966-2.1 profile or no profile leaves samples unchanged;
//! any other embedded ICC profile is converted to sRGB (relative colorimetric, MoxCms)
//! row by row. EXIF orientation other than 1 is refused rather than silently applied.
//! Progressive and non-baseline JPEGs are refused: zenjpeg buffers them whole.
//!
//! `jpeg-grid COLS ROWS PART... FRESH.rgb FRESH.json` composes baseline JPEG parts, given in
//! row-major order, into one raster by plain pixel placement (no resampling): every part of a grid
//! row must share its height and every grid column its width. Each part is decoded as above and the
//! record lists each part's SHA-256 and size. Parts must carry no ICC (or all the same sRGB one).
//!
//! `tiff SOURCE.tif FRESH.rgb FRESH.json` streams an uncompressed chunky 8-bit RGB TIFF or BigTIFF
//! (strips read in order; no predictor, no extra samples, orientation 1) row by row; an embedded
//! ICC profile other than sRGB IEC61966-2.1 is converted to sRGB (MoxCms, relative colorimetric).
//!
//! The `tiff` mode also reads tiled (cloud-optimized) TIFFs: 8-bit RGB or unassociated-alpha RGBA,
//! Deflate with optional horizontal predictor, one tile row at a time; RGBA is composited over black
//! and the alpha counts are recorded.
//!
//! `raw-grid COLS ROWS PART_W PART_H PART.rgb... FRESH.rgb FRESH.json` places equal-sized raw RGB8
//! rasters (row-major, for example the output of `tiff`) into one raster without resampling.
//!
//! `png SOURCE.png FRESH.rgb FRESH.json` streams a non-interlaced 8-bit RGB PNG through
//! zenpng's pull decoder (zencodec `StreamingDecode`). Colour chunks (iCCP, sRGB, gAMA, cHRM,
//! cICP) are refused rather than guessed at; an untagged PNG is treated as sRGB. The pull
//! decoder skips critical-chunk CRC and zlib checksum checks, so the source SHA-256 (and the
//! publisher's SHA-1 where one exists) is the integrity check.
//!
//! `thumb LEVEL.rgb WIDTH HEIGHT MAX_SIDE FRESH.jpg` makes the gallery preview from an
//! already prepared level raster (zenresize linear-light Lanczos, zenjpeg q85 4:2:0 baseline),
//! so previews are our own rendering of the data the viewer shows, never a third-party file.
#![forbid(unsafe_code)]
use imgref::ImgRefMut;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use zenpixels_convert::cms::PluggableCms;
use zenpixels_convert::cms_moxcms::MoxCms;
use zenpixels_convert::{ColorProfileSource, ConvertOptions, NamedProfile, PixelFormat};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const STRIP_ROWS: usize = 64;
const SRGB_MARKER: &[u8] = b"sRGB IEC61966-2.1";

struct Markers {
    icc: Vec<u8>,
    orientation: Option<u16>,
    sof: u8,
    components: u8,
    sampling: Vec<u8>,
}

fn u16be(b: &[u8]) -> usize {
    usize::from(u16::from_be_bytes([b[0], b[1]]))
}

fn orientation(exif: &[u8]) -> Option<u16> {
    let t = exif.strip_prefix(b"Exif\0\0")?;
    let le = match t.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = t.get(o..o + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32at = |o: usize| -> Option<usize> {
        let b: [u8; 4] = t.get(o..o + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        } as usize)
    };
    let ifd = u32at(4)?;
    for k in 0..usize::from(u16at(ifd)?) {
        let at = ifd + 2 + 12 * k;
        if u16at(at)? == 0x0112 {
            return u16at(at + 8);
        }
    }
    None
}

fn markers(data: &[u8]) -> Result<Markers> {
    if data.get(..2) != Some(&[0xff, 0xd8]) {
        return Err("not a JPEG".into());
    }
    let mut m = Markers {
        icc: Vec::new(),
        orientation: None,
        sof: 0,
        components: 0,
        sampling: Vec::new(),
    };
    let mut chunks: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut at = 2;
    while at + 4 <= data.len() {
        if data[at] != 0xff {
            return Err("JPEG marker framing".into());
        }
        let marker = data[at + 1];
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) || marker == 0xff {
            at += if marker == 0xff { 1 } else { 2 };
            continue;
        }
        let len = u16be(&data[at + 2..]);
        let seg = data
            .get(at + 4..at + 2 + len)
            .ok_or("JPEG segment extent")?;
        match marker {
            0xe2 if seg.starts_with(b"ICC_PROFILE\0") && seg.len() > 14 => {
                chunks.push((seg[12], seg[14..].to_vec()))
            }
            0xe1 if seg.starts_with(b"Exif\0\0") => {
                m.orientation = orientation(seg).or(m.orientation)
            }
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                m.sof = marker;
                m.components = seg[5];
                m.sampling = (0..usize::from(seg[5])).map(|k| seg[7 + 3 * k]).collect();
            }
            0xda => break,
            _ => {}
        }
        at += 2 + len;
    }
    chunks.sort_by_key(|c| c.0);
    m.icc = chunks.into_iter().flat_map(|c| c.1).collect();
    Ok(m)
}

fn fresh(path: &str) -> std::io::Result<std::fs::File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn jpeg(source: &str, raster: &str, record: &str) -> Result<()> {
    let data = std::fs::read(source)?;
    let source_sha = hex(&Sha256::digest(&data));
    let m = markers(&data)?;
    if m.sof != 0xc0 && m.sof != 0xc1 {
        return Err(format!("baseline sequential JPEG required (SOF{:x})", m.sof & 0xf).into());
    }
    if m.components != 3 {
        return Err("three-component JPEG required".into());
    }
    if matches!(m.orientation, Some(o) if o != 1) {
        return Err("EXIF orientation is not 1".into());
    }
    let srgb = m.icc.windows(SRGB_MARKER.len()).any(|w| w == SRGB_MARKER);
    let mut transform = if m.icc.is_empty() || srgb {
        None
    } else {
        Some(
            MoxCms
                .build_source_transform(
                    ColorProfileSource::Icc(&m.icc),
                    ColorProfileSource::Named(NamedProfile::Srgb),
                    PixelFormat::Rgb8,
                    PixelFormat::Rgb8,
                    &ConvertOptions::permissive(),
                )
                .ok_or("CMS declined")??,
        )
    };
    // Streaming MCU strips keep memory bounded by the compressed file plus a few strips, so the
    // 120 MP default pixel limit (a whole-image guard) is raised to the 3-gigapixel import bound.
    let mut reader = zenjpeg::decoder::Decoder::new()
        .max_pixels(3_000_000_000)
        .scanline_reader(&data)?;
    let (w, h) = (reader.width() as usize, reader.height() as usize);
    let mut out = BufWriter::new(fresh(raster)?);
    let mut raw_hash = Sha256::new();
    let mut strip = vec![0u8; w * 3 * STRIP_ROWS];
    let mut converted = if transform.is_some() {
        vec![0u8; w * 3]
    } else {
        Vec::new()
    };
    let mut rows = 0;
    while !reader.is_finished() {
        let n =
            reader.read_rows_rgb8(ImgRefMut::new(&mut strip, w * 3, STRIP_ROWS.min(h - rows)))?;
        if n == 0 {
            return Err("decoder returned no rows".into());
        }
        for row in strip[..n * w * 3].chunks_exact_mut(w * 3) {
            if let Some(t) = transform.as_mut() {
                t.transform_row(row, &mut converted, w as u32);
                row.copy_from_slice(&converted);
            }
        }
        raw_hash.update(&strip[..n * w * 3]);
        out.write_all(&strip[..n * w * 3])?;
        rows += n;
        if rows % (STRIP_ROWS * 64) == 0 {
            eprintln!("decoded {rows}/{h} rows");
        }
    }
    if rows != h {
        return Err("decoded row count".into());
    }
    out.flush()?;
    let colour = if m.icc.is_empty() {
        "no embedded ICC; samples treated as sRGB"
    } else if srgb {
        "embedded sRGB IEC61966-2.1 ICC; samples unchanged"
    } else {
        "embedded ICC converted to sRGB (MoxCms relative colorimetric)"
    };
    let json = format!(
        "{{\n \"source_sha256\": \"{source_sha}\",\n \"source_bytes\": {},\n \"width\": {w},\n \"height\": {h},\n \"raw_sha256\": \"{}\",\n \"raw_bytes\": {},\n \"jpeg_sampling\": {:?},\n \"icc_bytes\": {},\n \"icc_sha256\": \"{}\",\n \"exif_orientation\": {},\n \"color\": \"{colour}\"\n}}\n",
        data.len(),
        hex(&raw_hash.finalize()),
        w * h * 3,
        m.sampling,
        m.icc.len(),
        hex(&Sha256::digest(&m.icc)),
        m.orientation.map_or("null".to_string(), |o| o.to_string()),
    );
    fresh(record)?.write_all(json.as_bytes())?;
    println!("{w}x{h}: {colour}");
    Ok(())
}

fn jpeg_grid(cols: usize, rows: usize, parts: &[String], raster: &str, record: &str) -> Result<()> {
    if cols == 0 || rows == 0 || parts.len() != cols * rows {
        return Err("grid dimensions and part count".into());
    }
    let datas: Vec<Vec<u8>> = parts
        .iter()
        .map(std::fs::read)
        .collect::<std::io::Result<_>>()?;
    let mut info = Vec::new();
    for (path, data) in parts.iter().zip(&datas) {
        let m = markers(data)?;
        if (m.sof != 0xc0 && m.sof != 0xc1) || m.components != 3 {
            return Err(format!("{path}: baseline three-component JPEG required").into());
        }
        if matches!(m.orientation, Some(o) if o != 1) {
            return Err(format!("{path}: EXIF orientation is not 1").into());
        }
        let srgb = m.icc.windows(SRGB_MARKER.len()).any(|w| w == SRGB_MARKER);
        if !m.icc.is_empty() && !srgb {
            return Err(format!("{path}: non-sRGB ICC; grid import does not convert").into());
        }
        info.push((
            hex(&Sha256::digest(data)),
            data.len(),
            m.icc.len(),
            m.sampling,
        ));
    }
    let mut out = BufWriter::new(fresh(raster)?);
    let mut raw_hash = Sha256::new();
    let (mut total_w, mut total_h, mut written) = (0usize, 0usize, 0usize);
    let mut widths: Vec<usize> = Vec::new();
    for gr in 0..rows {
        let mut readers = Vec::new();
        for gc in 0..cols {
            readers.push(
                zenjpeg::decoder::Decoder::new()
                    .max_pixels(3_000_000_000)
                    .scanline_reader(&datas[gr * cols + gc])?,
            );
        }
        let band_h = readers[0].height() as usize;
        let row_widths: Vec<usize> = readers.iter().map(|r| r.width() as usize).collect();
        if readers.iter().any(|r| r.height() as usize != band_h) {
            return Err(format!("grid row {gr}: parts differ in height").into());
        }
        if gr == 0 {
            widths = row_widths.clone();
            total_w = widths.iter().sum();
        } else if row_widths != widths {
            return Err(format!("grid row {gr}: column widths differ from row 0").into());
        }
        let mut strips: Vec<Vec<u8>> = widths
            .iter()
            .map(|w| vec![0u8; w * 3 * STRIP_ROWS])
            .collect();
        let mut done = 0;
        while done < band_h {
            let take = STRIP_ROWS.min(band_h - done);
            for (k, reader) in readers.iter_mut().enumerate() {
                let n =
                    reader.read_rows_rgb8(ImgRefMut::new(&mut strips[k], widths[k] * 3, take))?;
                if n != take {
                    return Err("decoder returned a short strip".into());
                }
            }
            let mut line = vec![0u8; total_w * 3];
            for y in 0..take {
                let mut at = 0;
                for (k, w) in widths.iter().enumerate() {
                    line[at..at + w * 3].copy_from_slice(&strips[k][y * w * 3..(y + 1) * w * 3]);
                    at += w * 3;
                }
                raw_hash.update(&line);
                out.write_all(&line)?;
            }
            done += take;
            written += take;
        }
        total_h += band_h;
        eprintln!("grid row {gr}: {written}/… rows written");
    }
    out.flush()?;
    let parts_json: Vec<String> = parts
        .iter()
        .zip(&info)
        .map(|(path, (sha, bytes, icc, sampling))| {
            format!(" {{\"file\": \"{path}\", \"sha256\": \"{sha}\", \"bytes\": {bytes}, \"icc_bytes\": {icc}, \"sampling\": {sampling:?}}}")
        })
        .collect();
    let json = format!(
        "{{\n \"grid\": [{cols}, {rows}],\n \"width\": {total_w},\n \"height\": {total_h},\n \"raw_sha256\": \"{}\",\n \"raw_bytes\": {},\n \"exif_orientation\": 1,\n \"color\": \"parts without ICC (or sRGB IEC61966-2.1); samples treated as sRGB; parts placed without resampling\",\n \"parts\": [\n{}\n ]\n}}\n",
        hex(&raw_hash.finalize()),
        total_w * total_h * 3,
        parts_json.join(",\n"),
    );
    fresh(record)?.write_all(json.as_bytes())?;
    println!("{total_w}x{total_h}: {cols}x{rows} grid of baseline JPEG parts");
    Ok(())
}

fn tiff(source: &str, raster: &str, record: &str) -> Result<()> {
    use std::io::{BufReader, Read, Seek, SeekFrom};
    use tiff::decoder::{Decoder, DecodingResult};
    use tiff::tags::Tag;
    use tiff::{ColorType, decoder::ChunkType};
    // The tiff crate (image-tiff, also zentiff's engine) parses IFDs and decodes compressed strips/tiles;
    // zentiff itself returns whole images, so chunk streaming goes through the crate it wraps. Only
    // uncompressed strips are read directly: a single 15 GB strip must not be buffered as one chunk.
    let open = || -> Result<Decoder<BufReader<std::fs::File>>> {
        Ok(Decoder::new(BufReader::new(std::fs::File::open(source)?))?)
    };
    let mut dec = open()?;
    let source_len = std::fs::metadata(source)?.len();
    let (w, h) = dec.dimensions()?;
    let (w, h) = (w as usize, h as usize);
    let spp = match dec.colortype()? {
        ColorType::RGB(8) => 3usize,
        ColorType::RGBA(8) => 4,
        other => return Err(format!("8-bit RGB or RGBA TIFF required, found {other:?}").into()),
    };
    if spp == 4
        && dec
            .find_tag_unsigned_vec::<u16>(Tag::ExtraSamples)?
            .as_deref()
            != Some(&[2][..])
    {
        return Err("RGBA TIFF must carry unassociated alpha (ExtraSamples = 2)".into());
    }
    if dec
        .find_tag_unsigned::<u16>(Tag::Orientation)?
        .is_some_and(|o| o != 1)
    {
        return Err("TIFF orientation is not 1".into());
    }
    if dec
        .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)?
        .is_some_and(|p| p != 1)
    {
        return Err("chunky (interleaved) samples required".into());
    }
    let compression = dec.find_tag_unsigned::<u16>(Tag::Compression)?.unwrap_or(1);
    let icc: Vec<u8> = dec
        .find_tag_unsigned_vec::<u8>(Tag::IccProfile)?
        .unwrap_or_default();
    let srgb = icc.windows(SRGB_MARKER.len()).any(|x| x == SRGB_MARKER);
    let mut transform = if icc.is_empty() || srgb {
        None
    } else {
        Some(
            MoxCms
                .build_source_transform(
                    ColorProfileSource::Icc(&icc),
                    ColorProfileSource::Named(NamedProfile::Srgb),
                    PixelFormat::Rgb8,
                    PixelFormat::Rgb8,
                    &ConvertOptions::permissive(),
                )
                .ok_or("CMS declined")??,
        )
    };
    let tiled = dec.get_chunk_type() == ChunkType::Tile;
    let (cw, ch) = dec.chunk_dimensions();
    let (cw, ch) = (cw as usize, ch as usize);
    let (across, down) = if tiled {
        (w.div_ceil(cw), h.div_ceil(ch))
    } else {
        (1, h.div_ceil(ch))
    };
    let direct_strips = !tiled && compression == 1;
    let (offsets, counts) = if direct_strips {
        (
            dec.find_tag_unsigned_vec::<u64>(Tag::StripOffsets)?
                .ok_or("StripOffsets")?,
            dec.find_tag_unsigned_vec::<u64>(Tag::StripByteCounts)?
                .ok_or("StripByteCounts")?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut raw_file = std::fs::File::open(source)?;
    let mut out = BufWriter::new(fresh(raster)?);
    let (mut raw_hash, mut source_hash) = (Sha256::new(), Sha256::new());
    let mut alpha_counts = [0u64; 3]; // alpha zero, 255, other
    // Decoded chunks are assembled in a band of one chunk row; directly read uncompressed strips use one row.
    let mut band = vec![0u8; w * if direct_strips { 1 } else { ch } * spp];
    let (mut rgb_row, mut converted) = (vec![0u8; w * 3], vec![0u8; w * 3]);
    let mut written = 0usize;
    for cy in 0..down {
        let rows = ch.min(h - cy * ch);
        if direct_strips {
            let (off, size) = (offsets[cy], counts[cy]);
            if size != (rows * w * spp) as u64
                || off.checked_add(size).is_none_or(|e| e > source_len)
            {
                return Err("TIFF strip extent".into());
            }
            raw_file.seek(SeekFrom::Start(off))?;
            // Rows are read one at a time so memory stays one row even for a single huge strip.
            for _ in 0..rows {
                raw_file.read_exact(&mut band)?;
                let row = &band[..];
                finish_row(
                    row,
                    spp,
                    &mut rgb_row,
                    &mut converted,
                    transform.as_mut(),
                    &mut alpha_counts,
                    &mut source_hash,
                    &mut raw_hash,
                    &mut out,
                )?;
                written += 1;
            }
        } else {
            // Strips (one chunk per row of chunks) decode sequentially with the one decoder; tiles of a
            // chunk row decode in parallel, one decoder per rayon thread, and are placed in the band.
            let chunks: Vec<(usize, Vec<u8>)> = if !tiled {
                match dec.read_chunk(cy as u32)? {
                    DecodingResult::U8(v) => vec![(0, v)],
                    _ => return Err("chunk is not 8-bit".into()),
                }
            } else {
                (0..across)
                    .into_par_iter()
                    .map_init(
                        || open().map_err(|e| e.to_string()),
                        |dec, cx| -> std::result::Result<(usize, Vec<u8>), String> {
                            let dec = dec.as_mut().map_err(|e| e.clone())?;
                            let index = (cy * across + cx) as u32;
                            match dec
                                .read_chunk(index)
                                .map_err(|e| format!("chunk {index}: {e}"))?
                            {
                                DecodingResult::U8(v) => Ok((cx, v)),
                                _ => Err("chunk is not 8-bit".to_string()),
                            }
                        },
                    )
                    .collect::<std::result::Result<_, String>>()?
            };
            for (cx, data) in &chunks {
                let cols = cw.min(w - cx * cw);
                // A tile's data is cropped to the image at the right/bottom edges; strips span the width.
                let data_w = if tiled { cols } else { w };
                if data.len() != data_w * rows * spp {
                    return Err("decoded chunk size".into());
                }
                for y in 0..rows {
                    band[(y * w + cx * cw) * spp..(y * w + cx * cw + cols) * spp]
                        .copy_from_slice(&data[y * data_w * spp..(y * data_w + cols) * spp]);
                }
            }
            for y in 0..rows {
                let row = &band[y * w * spp..(y + 1) * w * spp];
                finish_row(
                    row,
                    spp,
                    &mut rgb_row,
                    &mut converted,
                    transform.as_mut(),
                    &mut alpha_counts,
                    &mut source_hash,
                    &mut raw_hash,
                    &mut out,
                )?;
                written += 1;
            }
        }
        if cy % 64 == 0 {
            eprintln!("converted {written}/{h} rows");
        }
    }
    if written != h {
        return Err("TIFF row count".into());
    }
    out.flush()?;
    let colour = if icc.is_empty() {
        "no embedded ICC; samples treated as sRGB"
    } else if srgb {
        "embedded sRGB IEC61966-2.1 ICC; samples unchanged"
    } else {
        "embedded ICC converted to sRGB (MoxCms relative colorimetric)"
    };
    let alpha = if spp == 4 {
        format!(
            "RGBA composited over black (alpha 0: {}, 255: {}, partial: {})",
            alpha_counts[0], alpha_counts[1], alpha_counts[2]
        )
    } else {
        "no alpha".to_string()
    };
    let json = format!(
        "{{\n \"source_bytes\": {source_len},\n \"width\": {w},\n \"height\": {h},\n \"chunk\": [{cw}, {ch}, \"{}\"],\n \"compression\": {compression},\n \"samples_per_pixel\": {spp},\n \"source_pixel_sha256\": \"{}\",\n \"raw_sha256\": \"{}\",\n \"raw_bytes\": {},\n \"icc_bytes\": {},\n \"icc_sha256\": \"{}\",\n \"alpha\": \"{alpha}\",\n \"color\": \"{colour}\"\n}}\n",
        if tiled { "tile" } else { "strip" },
        hex(&source_hash.finalize()),
        hex(&raw_hash.finalize()),
        w * h * 3,
        icc.len(),
        hex(&Sha256::digest(&icc)),
    );
    fresh(record)?.write_all(json.as_bytes())?;
    println!("{w}x{h}: {colour}; {alpha}");
    Ok(())
}

/// One source row (RGB or unassociated RGBA) to the output RGB8 row: hashes, alpha over black, ICC to sRGB.
#[allow(clippy::too_many_arguments)]
fn finish_row(
    row: &[u8],
    spp: usize,
    rgb_row: &mut [u8],
    converted: &mut [u8],
    transform: Option<&mut Box<dyn zenpixels_convert::cms::RowTransformMut>>,
    alpha_counts: &mut [u64; 3],
    source_hash: &mut Sha256,
    raw_hash: &mut Sha256,
    out: &mut impl Write,
) -> Result<()> {
    source_hash.update(row);
    if spp == 4 {
        for (x, px) in row.chunks_exact(4).enumerate() {
            let a = px[3];
            alpha_counts[if a == 0 {
                0
            } else if a == 255 {
                1
            } else {
                2
            }] += 1;
            for c in 0..3 {
                rgb_row[x * 3 + c] = ((u32::from(px[c]) * u32::from(a) + 127) / 255) as u8;
            }
        }
    } else {
        rgb_row.copy_from_slice(row);
    }
    let line: &[u8] = if let Some(t) = transform {
        t.transform_row(rgb_row, converted, (rgb_row.len() / 3) as u32);
        converted
    } else {
        rgb_row
    };
    raw_hash.update(line);
    out.write_all(line)?;
    Ok(())
}

fn raw_grid(
    cols: usize,
    rows: usize,
    pw: usize,
    ph: usize,
    parts: &[String],
    raster: &str,
    record: &str,
) -> Result<()> {
    use std::io::Read;
    if cols == 0 || rows == 0 || parts.len() != cols * rows {
        return Err("grid dimensions and part count".into());
    }
    let mut files = Vec::new();
    let mut part_hashes = Vec::new();
    for p in parts {
        let f = std::fs::File::open(p)?;
        if f.metadata()?.len() != (pw * ph * 3) as u64 {
            return Err(format!("{p}: raster extent").into());
        }
        files.push(f);
        part_hashes.push(Sha256::new());
    }
    let mut out = BufWriter::new(fresh(raster)?);
    let mut raw_hash = Sha256::new();
    let mut row = vec![0u8; pw * 3];
    for gr in 0..rows {
        for _ in 0..ph {
            for gc in 0..cols {
                files[gr * cols + gc].read_exact(&mut row)?;
                part_hashes[gr * cols + gc].update(&row);
                raw_hash.update(&row);
                out.write_all(&row)?;
            }
        }
        eprintln!("grid row {gr} placed");
    }
    out.flush()?;
    let list: Vec<String> = parts
        .iter()
        .zip(part_hashes)
        .map(|(p, h)| {
            format!(
                " {{\"file\": \"{p}\", \"raw_sha256\": \"{}\"}}",
                hex(&h.finalize())
            )
        })
        .collect();
    let json = format!(
        "{{\n \"grid\": [{cols}, {rows}],\n \"width\": {},\n \"height\": {},\n \"raw_sha256\": \"{}\",\n \"raw_bytes\": {},\n \"color\": \"parts placed without resampling; see each part's import record\",\n \"parts\": [\n{}\n ]\n}}\n",
        pw * cols,
        ph * rows,
        hex(&raw_hash.finalize()),
        pw * cols * ph * rows * 3,
        list.join(",\n")
    );
    fresh(record)?.write_all(json.as_bytes())?;
    println!(
        "{}x{}: {cols}x{rows} grid of raw parts",
        pw * cols,
        ph * rows
    );
    Ok(())
}

fn png(source: &str, raster: &str, record: &str) -> Result<()> {
    use zencodec_png::decode::{DecodeJob, DecoderConfig, StreamingDecode};
    let data = std::fs::read(source)?;
    let source_sha = hex(&Sha256::digest(&data));
    if data.get(..8) != Some(b"\x89PNG\r\n\x1a\n".as_slice())
        || data.get(12..16) != Some(b"IHDR".as_slice())
    {
        return Err("not a PNG".into());
    }
    let (w, h) = (
        u32::from_be_bytes(data[16..20].try_into()?) as usize,
        u32::from_be_bytes(data[20..24].try_into()?) as usize,
    );
    if data[24] != 8 || data[25] != 2 || data[28] != 0 {
        return Err("non-interlaced 8-bit RGB PNG required".into());
    }
    // Chunk walk: refuse colour-management chunks, count IDAT.
    let (mut at, mut idat) = (8usize, 0u64);
    while at + 12 <= data.len() {
        let len = u32::from_be_bytes(data[at..at + 4].try_into()?) as usize;
        let kind = &data[at + 4..at + 8];
        if matches!(kind, b"iCCP" | b"sRGB" | b"gAMA" | b"cHRM" | b"cICP") {
            return Err(format!(
                "colour chunk {} present; not guessed",
                String::from_utf8_lossy(kind)
            )
            .into());
        }
        if kind == b"IDAT" {
            idat += 1;
        }
        at += 12 + len;
    }
    // Row streaming: memory is the compressed file (plus the inflate thread's copy) and a few rows,
    // so the default whole-image pixel guard is raised to the 3-gigapixel import bound.
    let mut decoder = zenpng::PngDecoderConfig::new()
        .job()
        .with_limits(zencodec_png::ResourceLimits::none().with_max_pixels(3_000_000_000))
        .streaming_decoder(data.into(), &[])?;
    let mut out = BufWriter::new(fresh(raster)?);
    let mut raw_hash = Sha256::new();
    let mut rows = 0;
    while let Some((_, batch)) = decoder.next_batch()? {
        for y in 0..batch.rows() {
            let row = batch.row(y);
            if row.len() != w * 3 {
                return Err("decoded row is not RGB8".into());
            }
            raw_hash.update(row);
            out.write_all(row)?;
            rows += 1;
        }
        if rows % 4096 < batch.rows() as usize {
            eprintln!("decoded {rows}/{h} rows");
        }
    }
    if rows != h {
        return Err("decoded row count".into());
    }
    out.flush()?;
    let json = format!(
        "{{\n \"source_sha256\": \"{source_sha}\",\n \"width\": {w},\n \"height\": {h},\n \"raw_sha256\": \"{}\",\n \"raw_bytes\": {},\n \"idat_chunks\": {idat},\n \"color\": \"no colour chunks; samples treated as sRGB\"\n}}\n",
        hex(&raw_hash.finalize()),
        w * h * 3,
    );
    fresh(record)?.write_all(json.as_bytes())?;
    println!("{w}x{h}: 8-bit RGB PNG, no colour chunks; samples treated as sRGB");
    Ok(())
}

fn thumb(level: &str, w: u32, h: u32, max_side: u32, out: &str) -> Result<()> {
    use zenresize::{Filter, PixelDescriptor, ResizeConfig, StreamingResize};
    let raw = std::fs::read(level)?;
    if raw.len() as u64 != u64::from(w) * u64::from(h) * 3 {
        return Err("level raster extent".into());
    }
    let scale = f64::from(max_side) / f64::from(w.max(h));
    let (tw, th) = if scale >= 1.0 {
        return Err("thumbnail would upscale".into());
    } else {
        (
            ((f64::from(w) * scale).round() as u32).max(1),
            ((f64::from(h) * scale).round() as u32).max(1),
        )
    };
    let config = ResizeConfig::builder(w, h, tw, th)
        .filter(Filter::Lanczos)
        .format(PixelDescriptor::RGB8_SRGB)
        .linear()
        .build();
    let mut resize = StreamingResize::new(&config);
    let mut small = Vec::with_capacity(tw as usize * th as usize * 3);
    for row in raw.chunks_exact(w as usize * 3) {
        resize.push_row(row)?;
        while let Some(r) = resize.next_output_row() {
            small.extend_from_slice(r);
        }
    }
    for _ in 0..resize.finish() {
        small.extend_from_slice(resize.next_output_row().ok_or("resize drain")?);
    }
    if small.len() != tw as usize * th as usize * 3 {
        return Err("thumbnail extent".into());
    }
    let mut e =
        zenjpeg::encoder::EncoderConfig::ycbcr(85.0, zenjpeg::encoder::ChromaSubsampling::Quarter)
            .progressive(false)
            .encode_from_bytes(tw, th, zenjpeg::encoder::PixelLayout::Rgb8Srgb)?;
    e.push_packed(&small, zenjpeg::encoder::Unstoppable)?;
    let bytes = e.finish()?;
    fresh(out)?.write_all(&bytes)?;
    println!(
        "{{\"width\":{tw},\"height\":{th},\"bytes\":{},\"sha256\":\"{}\"}}",
        bytes.len(),
        hex(&Sha256::digest(&bytes))
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("jpeg") if a.len() == 5 => jpeg(&a[2], &a[3], &a[4]),
        Some("jpeg-grid") if a.len() > 6 => {
            let n = a.len();
            jpeg_grid(a[2].parse()?, a[3].parse()?, &a[4..n - 2], &a[n - 2], &a[n - 1])
        }
        Some("raw-grid") if a.len() > 9 => {
            let n = a.len();
            raw_grid(a[2].parse()?, a[3].parse()?, a[4].parse()?, a[5].parse()?, &a[6..n - 2], &a[n - 2], &a[n - 1])
        }
        Some("tiff") if a.len() == 5 => tiff(&a[2], &a[3], &a[4]),
        Some("png") if a.len() == 5 => png(&a[2], &a[3], &a[4]),
        Some("thumb") if a.len() == 7 => thumb(&a[2], a[3].parse()?, a[4].parse()?, a[5].parse()?, &a[6]),
        _ => Err("gallery_import jpeg SOURCE.jpg FRESH.rgb FRESH.json | raw-grid COLS ROWS PW PH PART.rgb... FRESH.rgb FRESH.json | tiff SOURCE.tif FRESH.rgb FRESH.json | jpeg-grid COLS ROWS PART... FRESH.rgb FRESH.json | png SOURCE.png FRESH.rgb FRESH.json | thumb LEVEL.rgb W H MAX_SIDE FRESH.jpg".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tiny_tiff(big: bool, le: bool, rows_per_strip: u32, bits: [u16; 3]) -> Vec<u8> {
        // 3 x 2 RGB image, two rows; strips placed after the IFD.
        let pixels: Vec<u8> = (0..18).collect();
        let w = |v: u64, n: usize| -> Vec<u8> {
            let b = v.to_le_bytes();
            let mut o = b[..n].to_vec();
            if !le {
                o.reverse();
            }
            o
        };
        let strips = 2u32.div_ceil(rows_per_strip) as usize;
        let (hdr, entry, word) = if big {
            (16usize, 20usize, 8usize)
        } else {
            (8, 12, 4)
        };
        let tags: Vec<(u16, u16, u64, u64)> = vec![
            (256, 4, 1, 3),
            (257, 4, 1, 2),
            (258, 3, 3, 0),
            (259, 3, 1, 1),
            (262, 3, 1, 2),
            (273, 4, strips as u64, 0),
            (277, 3, 1, 3),
            (278, 4, 1, u64::from(rows_per_strip)),
            (279, 4, strips as u64, 0),
        ];
        let ifd_len = (if big { 8 } else { 2 }) + tags.len() * entry + word;
        let extra = hdr + ifd_len;
        // Out-of-line values: bits (6 bytes), strip offsets/sizes when more than one strip.
        let mut blob: Vec<u8> = Vec::new();
        let bits_at = extra;
        for b in bits {
            blob.extend(w(u64::from(b), 2));
        }
        let off_at = extra + blob.len();
        let data_at = off_at + strips * 4 * 2;
        let row_bytes = 9 * rows_per_strip as usize;
        let mut offs = Vec::new();
        let mut sizes = Vec::new();
        for i in 0..strips {
            offs.extend(w((data_at + i * row_bytes) as u64, 4));
            sizes.extend(w((row_bytes.min(18 - i * row_bytes)) as u64, 4));
        }
        let blob_bits = blob.clone();
        let offs_inline = offs.clone();
        let sizes_inline = sizes.clone();
        let mut out = if le { b"II".to_vec() } else { b"MM".to_vec() };
        out.extend(w(if big { 43 } else { 42 }, 2));
        if big {
            out.extend(w(8, 2));
            out.extend(w(0, 2));
            out.extend(w(16, 8));
            out.extend(w(tags.len() as u64, 8));
        } else {
            out.extend(w(8, 4));
            out.extend(w(tags.len() as u64, 2));
        }
        for (tag, typ, cnt, val) in tags {
            out.extend(w(u64::from(tag), 2));
            out.extend(w(u64::from(typ), 2));
            out.extend(w(cnt, if big { 8 } else { 4 }));
            // Values that fit the entry's value field are stored inline (BigTIFF: 8 bytes).
            let mut v = match tag {
                258 if 6 <= word => blob_bits.clone(),
                258 => w(bits_at as u64, word),
                273 if strips * 4 <= word => offs_inline.clone(),
                273 => w(off_at as u64, word),
                279 if strips * 4 <= word => sizes_inline.clone(),
                279 => w((off_at + strips * 4) as u64, word),
                _ => w(val, if typ == 3 { 2 } else { 4 }),
            };
            v.resize(word, 0);
            out.extend(v);
        }
        out.extend(w(0, word));
        assert_eq!(out.len(), extra);
        out.extend(blob);
        out.extend(offs);
        out.extend(sizes);
        out.extend(&pixels);
        out
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &d in data {
            a = (a + u32::from(d)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// A 5 x 3 RGBA image in 4 x 2 tiles (partial edge tiles), horizontal predictor, stored-block zlib.
    fn tiled_fixture(alpha: [u8; 15]) -> (Vec<u8>, Vec<u8>) {
        let (w, h, tw, th, spp) = (5usize, 3usize, 4usize, 2usize, 4usize);
        let pixel = |x: usize, y: usize| -> [u8; 4] {
            [
                (x * 40 + y) as u8,
                (y * 70 + x) as u8,
                (x * 9 + y * 5 + 100) as u8,
                alpha[y * w + x],
            ]
        };
        let mut tiles = Vec::new();
        for ty in 0..h.div_ceil(th) {
            for tx in 0..w.div_ceil(tw) {
                let mut raw = vec![0u8; tw * th * spp];
                for y in 0..th {
                    for x in 0..tw {
                        let (gx, gy) = (tx * tw + x, ty * th + y);
                        let p = if gx < w && gy < h {
                            pixel(gx, gy)
                        } else {
                            [7, 8, 9, 10]
                        };
                        raw[(y * tw + x) * spp..(y * tw + x + 1) * spp].copy_from_slice(&p);
                    }
                }
                // forward horizontal differencing, then a stored-block zlib stream
                for row in raw.chunks_exact_mut(tw * spp) {
                    for x in (spp..row.len()).rev() {
                        row[x] = row[x].wrapping_sub(row[x - spp]);
                    }
                }
                let mut z = vec![0x78, 0x01, 0x01];
                z.extend((raw.len() as u16).to_le_bytes());
                z.extend((!(raw.len() as u16)).to_le_bytes());
                z.extend(&raw);
                z.extend(adler32(&raw).to_be_bytes());
                tiles.push(z);
            }
        }
        // Little-endian classic TIFF: header, IFD, then bits/tile arrays and tile data.
        let tags: [(u16, u16, u32, u32); 13] = [
            (256, 4, 1, w as u32),
            (257, 4, 1, h as u32),
            (258, 3, 4, 0),
            (259, 3, 1, 8),
            (262, 3, 1, 2),
            (277, 3, 1, 4),
            (317, 3, 1, 2),
            (322, 4, 1, tw as u32),
            (323, 4, 1, th as u32),
            (324, 4, tiles.len() as u32, 0),
            (325, 4, tiles.len() as u32, 0),
            (338, 3, 1, 2),
            (339, 3, 4, 0),
        ];
        let ifd_at = 8usize;
        let after_ifd = ifd_at + 2 + tags.len() * 12 + 4;
        let (bits_at, sf_at) = (after_ifd, after_ifd + 8);
        let (offs_at, counts_at) = (sf_at + 8, sf_at + 8 + tiles.len() * 4);
        let mut data_at = counts_at + tiles.len() * 4;
        let mut offs = Vec::new();
        for t in &tiles {
            offs.push(data_at as u32);
            data_at += t.len();
        }
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((tags.len() as u16).to_le_bytes());
        for (tag, typ, cnt, val) in tags {
            out.extend(tag.to_le_bytes());
            out.extend(typ.to_le_bytes());
            out.extend(cnt.to_le_bytes());
            let v = match tag {
                258 => bits_at as u32,
                324 => offs_at as u32,
                325 => counts_at as u32,
                339 => sf_at as u32,
                _ => val,
            };
            if matches!(typ, 3) && cnt == 1 {
                out.extend((v as u16).to_le_bytes());
                out.extend([0, 0]);
            } else {
                out.extend(v.to_le_bytes());
            }
        }
        out.extend(0u32.to_le_bytes());
        for _ in 0..4 {
            out.extend(8u16.to_le_bytes());
        }
        for _ in 0..4 {
            out.extend(1u16.to_le_bytes());
        }
        for o in &offs {
            out.extend(o.to_le_bytes());
        }
        for t in &tiles {
            out.extend((t.len() as u32).to_le_bytes());
        }
        for t in &tiles {
            out.extend(t);
        }
        let mut expected = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let p = pixel(x, y);
                for c in 0..3 {
                    expected.push(((u32::from(p[c]) * u32::from(p[3]) + 127) / 255) as u8);
                }
            }
        }
        (out, expected)
    }

    #[test]
    fn raw_grid_places_parts_row_major() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let id = std::process::id();
        // 2 x 2 parts of 2 x 2 pixels, each part filled with its own index.
        let mut names = Vec::new();
        for i in 0..4u8 {
            let n = dir.join(format!("gallery-import-grid-{id}-{i}.rgb"));
            std::fs::write(&n, vec![i; 2 * 2 * 3]).unwrap();
            names.push(n.to_str().unwrap().to_string());
        }
        let (out, rec) = (
            dir.join(format!("gallery-import-grid-{id}.rgb")),
            dir.join(format!("gallery-import-grid-{id}.json")),
        );
        raw_grid(
            2,
            2,
            2,
            2,
            &names,
            out.to_str().unwrap(),
            rec.to_str().unwrap(),
        )
        .unwrap();
        let got = std::fs::read(&out).unwrap();
        let expect: Vec<u8> = [[0u8, 1], [0, 1], [2, 3], [2, 3]]
            .iter()
            .flat_map(|r| r.iter().flat_map(|&v| [v; 6]))
            .collect();
        assert_eq!(got, expect);
        assert!(
            std::fs::read_to_string(&rec)
                .unwrap()
                .contains("\"width\": 4")
        );
        for f in names.iter().map(std::path::PathBuf::from).chain([out, rec]) {
            std::fs::remove_file(f).unwrap();
        }
    }

    #[test]
    fn tiled_rgba_deflate_predictor_composites_over_black() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let alpha: [u8; 15] = [
            255, 255, 0, 128, 255, 255, 0, 0, 255, 17, 255, 255, 255, 255, 1,
        ];
        let (bytes, expected) = tiled_fixture(alpha);
        let id = std::process::id();
        let src = dir.join(format!("gallery-import-tiled-{id}.tif"));
        let out = dir.join(format!("gallery-import-tiled-{id}.rgb"));
        let rec = dir.join(format!("gallery-import-tiled-{id}.json"));
        std::fs::write(&src, bytes).unwrap();
        tiff(
            src.to_str().unwrap(),
            out.to_str().unwrap(),
            rec.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), expected);
        let json = std::fs::read_to_string(&rec).unwrap();
        assert!(json.contains("alpha 0: 3, 255: 9, partial: 3"), "{json}");
        for f in [&src, &out, &rec] {
            std::fs::remove_file(f).unwrap();
        }
    }

    #[test]
    fn tiff_variants_stream_rows_and_refuse_unsupported() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        for (big, le, rps) in [
            (false, true, 2),
            (false, false, 1),
            (true, true, 1),
            (true, false, 2),
        ] {
            let id = std::process::id();
            let src = dir.join(format!("gallery-import-tiff-{id}-{big}-{le}-{rps}.tif"));
            let out = dir.join(format!("gallery-import-tiff-{id}-{big}-{le}-{rps}.rgb"));
            let rec = dir.join(format!("gallery-import-tiff-{id}-{big}-{le}-{rps}.json"));
            std::fs::write(&src, tiny_tiff(big, le, rps, [8, 8, 8])).unwrap();
            tiff(
                src.to_str().unwrap(),
                out.to_str().unwrap(),
                rec.to_str().unwrap(),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(&out).unwrap(),
                (0..18).collect::<Vec<u8>>(),
                "{big} {le} {rps}"
            );
            for f in [&src, &out, &rec] {
                std::fs::remove_file(f).unwrap();
            }
        }
        let src = dir.join(format!("gallery-import-tiff16-{}.tif", std::process::id()));
        std::fs::write(&src, tiny_tiff(false, true, 2, [16, 16, 16])).unwrap();
        let out = dir.join(format!("gallery-import-tiff16-{}.rgb", std::process::id()));
        let rec = dir.join(format!("gallery-import-tiff16-{}.json", std::process::id()));
        assert!(
            tiff(
                src.to_str().unwrap(),
                out.to_str().unwrap(),
                rec.to_str().unwrap()
            )
            .is_err()
        );
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn exif_orientation_both_byte_orders() {
        let mut le = b"Exif\0\0II*\0".to_vec();
        le.extend(8u32.to_le_bytes());
        le.extend(1u16.to_le_bytes());
        le.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        assert_eq!(orientation(&le), Some(6));
        let mut be = b"Exif\0\0MM\0*".to_vec();
        be.extend(8u32.to_be_bytes());
        be.extend(1u16.to_be_bytes());
        be.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 8, 0, 0]);
        assert_eq!(orientation(&be), Some(8));
        assert_eq!(orientation(b"Exif\0\0II*\0"), None);
    }
}
