//! Encoded-domain V4 XYB sRGB power transfer (quantizer bit 5) repacker.
//!
//! Upgrades existing V4 full-precision sRGB8 XYB lossy tiles (`bit 5 = 0`) to
//! `u8_dsqrt_d4` (`bit 5 = 1`) without re-encoding DCT or entropy bitstreams:
//!   - Flips quantizer bit 5 in the V4 lossy setup blob
//!   - Re-hashes V4 object/tile integrity tags and directory via `TileReader` / `TileWriter`
//!   - Recomputes `pSUM` (using `u8_dsqrt_d4`) and `cSUM`
//!   - Strictly verifies every repacked tile with `imij::decode`
//!
//! Supports both Explorer tile archives (`tiles IN.imijset OUT.imijset`) and
//! multi-level native V4 pyramids (`native IN.imi OUT_DIR`).

use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::planes::{
    ImageDirectory, ImageView, ImageViewRole, LevelDescriptor, PlaneDescriptor, PlaneSetLimits,
    PlaneSetReader, PlaneSetWriter, ReadAt, Rect, SeekReader, ViewBacking, ViewRegistration,
};
use imij::v4::{
    DirectoryPlacement, DirectoryPrefix, DirectoryWriter, Entry, EntryKind, ObjectIntegrity,
    SetupContext, TileEvent, TileLayout, TileLimits, TileOrder as V4TileOrder, TileReader,
    TileWriter, append_directory_prefix,
};
use imij::{Channels, Cicp, Coding, HashAlgorithm, Hasher, LossyColor, SampleType};
use imij_set::{IndexPlacement, PyramidCatalog, PyramidLevel, Reader, Summary, TileOrder, Writer};
use rayon::prelude::*;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const SIGNATURE: [u8; 8] = [b'I', b'M', b'I', b'J', 0x0D, 0x0A, 0x87, 0x0A];
const QUANTIZER_BIT_POWER: u8 = 1 << 5;
const UNIT_DC: u64 = 51;
const UNIT_ALPHA: u64 = 53;
const UNIT_AC: u64 = 55;
const UNIT_EXACT: u64 = 57;
const MODEL_DEFINITION: u64 = 63;

const TW: u32 = 512;
const TH: u32 = 256;
const MAX_TILE: usize = 16 << 20;
const PAGE: usize = 256;
const MAX_OPEN_BYTES: u64 = 256 << 10;
const MAX_OPEN_RANGES: usize = 8;

fn fresh(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let mut val = 0u64;
    let mut shift = 0u32;
    loop {
        if *pos >= buf.len() {
            return Err("unexpected EOF reading varint".into());
        }
        let b = buf[*pos];
        *pos += 1;
        val |= u64::from(b & 0x7f)
            .checked_shl(shift)
            .ok_or("varint overflow")?;
        if b & 0x80 == 0 {
            return Ok(val);
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint too long".into());
        }
    }
}

fn read_varint_reader<R: Read>(r: &mut R) -> Result<u64> {
    let mut val = 0u64;
    let mut shift = 0u32;
    loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b)?;
        val |= u64::from(b[0] & 0x7f)
            .checked_shl(shift)
            .ok_or("varint overflow")?;
        if b[0] & 0x80 == 0 {
            return Ok(val);
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint too long".into());
        }
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(tag);
    put_varint(out, body.len() as u64 + 1);
    out.extend_from_slice(body);
}

fn build_csum_chunk(head: &[u8], tail: &[u8], algo: HashAlgorithm, width: u8, k: u8) -> Vec<u8> {
    let covered_len = (head.len() + tail.len()) as u64;
    let piece_size = 1u64 << k;
    let pieces = covered_len.div_ceil(piece_size).max(1);
    let mut payload = Vec::with_capacity(16 + pieces as usize * width as usize);
    payload.extend_from_slice(&[algo.wire(), width, k]);
    put_varint(&mut payload, covered_len);
    for i in 0..pieces {
        let start = (i * piece_size) as usize;
        let end = ((i + 1) * piece_size).min(covered_len) as usize;
        let mut h = Hasher::new(algo);
        if end <= head.len() {
            h.update(&head[start..end]);
        } else if start >= head.len() {
            h.update(&tail[start - head.len()..end - head.len()]);
        } else {
            h.update(&head[start..]);
            h.update(&tail[..end - head.len()]);
        }
        let d = h.finish();
        payload.extend_from_slice(&d.as_bytes()[..width as usize]);
    }
    let mut chunk = Vec::with_capacity(4 + varint_len(payload.len() as u64 + 1) + payload.len());
    push_chunk(&mut chunk, b"cSUM", &payload);
    chunk
}

fn hash_algo_from_wire(wire: u8) -> Result<HashAlgorithm> {
    HashAlgorithm::from_wire(wire)
        .ok_or_else(|| format!("unsupported cSUM algorithm wire id {wire}").into())
}

/// Flip V4 quantizer bit 5 (`0 -> 1`) in a single V4 lossy sRGB8 XYB tile,
/// re-hashing V4 object integrity, directory, `cSUM`, and `pSUM`.
fn repack_v4_lossy_tile(input: &[u8]) -> Result<Vec<u8>> {
    let hdr = imij::header(input)?;
    if hdr.version != 4 {
        return Err(format!("expected V4 tile, got V{}", hdr.version).into());
    }
    let full_precision =
        (0..hdr.info.channels.count()).all(|c| hdr.info.channel_bits(c) == Some(8));
    if hdr.coding != Coding::Lossy
        || hdr.lossy_color != Some(LossyColor::Xyb)
        || hdr.info.sample != SampleType::U8
        || hdr.info.cicp != Cicp::SRGB
        || !full_precision
    {
        return Err("tile is not full-precision V4 sRGB8 XYB lossy".into());
    }

    // Parse header boundary: SIGNATURE (8B) + varint(header_len) + header_body
    if input.len() < 10 || input[..8] != SIGNATURE {
        return Err("bad IMIJ signature".into());
    }
    let mut pos = 8;
    let hlen = usize::try_from(read_varint(input, &mut pos)?)?;
    let header_end = pos.checked_add(hlen).ok_or("header length overflow")?;
    if header_end > input.len() {
        return Err("truncated header".into());
    }

    // Walk outer chunks
    let mut csum_params: Option<(HashAlgorithm, u8, u8)> = None;
    let mut had_psum = false;
    let mut data_body: Option<&[u8]> = None;
    let mut other_before_data: Vec<([u8; 4], &[u8])> = Vec::new();
    let mut other_after_data: Vec<([u8; 4], &[u8])> = Vec::new();

    let mut cur = header_end;
    while cur < input.len() {
        if cur + 4 > input.len() {
            return Err("truncated chunk tag".into());
        }
        let tag: [u8; 4] = input[cur..cur + 4].try_into().unwrap();
        cur += 4;
        let len_plus_one = read_varint(input, &mut cur)?;
        let body = if len_plus_one == 0 {
            let b = &input[cur..];
            cur = input.len();
            b
        } else {
            let len = usize::try_from(len_plus_one - 1)?;
            let end = cur.checked_add(len).ok_or("chunk overflow")?;
            if end > input.len() {
                return Err("truncated chunk body".into());
            }
            let b = &input[cur..end];
            cur = end;
            b
        };
        match &tag {
            b"cSUM" => {
                if body.len() < 4 {
                    return Err("truncated cSUM chunk".into());
                }
                csum_params = Some((hash_algo_from_wire(body[0])?, body[1], body[2]));
            }
            b"pSUM" => {
                had_psum = true;
            }
            b"uIDX" => {}
            b"dLOC" => {
                return Err("unexpected dLOC in single-tile image".into());
            }
            b"DATA" => {
                if data_body.is_some() {
                    return Err("duplicate DATA chunk".into());
                }
                data_body = Some(body);
            }
            _ => {
                if data_body.is_none() {
                    other_before_data.push((tag, body));
                } else {
                    other_after_data.push((tag, body));
                }
            }
        }
    }
    let data = data_body.ok_or("missing DATA chunk")?;

    // Parse V4 lossy setup, directory placement, and object bounds
    let session = DecodeSession::new(
        DecodePolicy::default()
            .with_output_limits(imij::Limits::NONE)
            .with_max_input_bytes(u64::MAX)
            .with_max_working_bytes(u64::MAX),
    );
    let tile_limits = TileLimits {
        max_object_bytes: MAX_TILE as u64,
        max_tile_bytes: MAX_TILE as u64,
    };

    // Locate setup bytes, parse layout & integrity, and flip quantizer bit 5
    let mut dp = 0usize;
    let setup_len = usize::try_from(read_varint(data, &mut dp)?)?;
    let old_setup = &data[dp..dp + setup_len];
    let dir_prefix = DirectoryPrefix::read(&data[dp + setup_len..], &session)?;
    let objects_at = dp + setup_len + dir_prefix.encoded_len;

    let mut sp = 0usize;
    let rows = u32::try_from(read_varint(old_setup, &mut sp)?)?;
    let columns = u32::try_from(read_varint(old_setup, &mut sp)?)?;
    let tile_columns = u32::try_from(read_varint(old_setup, &mut sp)?)?;
    let tile_rows = u32::try_from(read_varint(old_setup, &mut sp)?)?;
    let order = V4TileOrder::try_from(read_varint(old_setup, &mut sp)?)?;
    let integrity = ObjectIntegrity::try_from(read_varint(old_setup, &mut sp)?)?;
    let _coder = read_varint(old_setup, &mut sp)?;
    let groups = if columns == 0 {
        1
    } else {
        hdr.info.width.div_ceil(columns)
    };
    let stripes = hdr.info.height.div_ceil(8).div_ceil(rows);
    let layout = TileLayout::new(groups, stripes, tile_columns, tile_rows, order)?;

    let q_len = usize::try_from(read_varint(old_setup, &mut sp)?)?;
    if q_len == 0 || sp + q_len > old_setup.len() {
        return Err("malformed quantizer blob".into());
    }
    let present_pos = sp;
    let old_present = old_setup[present_pos];
    if old_present >= 0x80 {
        return Err("unexpected multi-byte quantizer presence mask".into());
    }
    if old_present & QUANTIZER_BIT_POWER != 0 {
        // Already upgraded; verify and return unchanged.
        let (dec_info, _) = imij::decode(input)?;
        if dec_info != hdr.info {
            return Err("already-upgraded tile geometry mismatch".into());
        }
        return Ok(input.to_vec());
    }

    let mut new_setup = old_setup.to_vec();
    new_setup[present_pos] = old_present | QUANTIZER_BIT_POWER;

    let old_context = SetupContext::from_image_declaration(
        &hdr.info,
        Coding::Lossy,
        Some(LossyColor::Xyb),
        old_setup,
    )?;
    let new_context = SetupContext::from_image_declaration(
        &hdr.info,
        Coding::Lossy,
        Some(LossyColor::Xyb),
        &new_setup,
    )?;

    // Stream all V4 objects through TileReader -> TileWriter + DirectoryWriter
    let objects = &data[objects_at..];
    let mut reader = TileReader::new(layout, integrity, old_context, tile_limits, &session);
    let mut writer = TileWriter::new(layout, integrity, new_context, tile_limits);
    let mut directory = DirectoryWriter::new(layout, integrity);
    let mut new_objects = Vec::with_capacity(objects.len());
    let mut dir_entries = Vec::new();

    let mut flush = |writer: &mut TileWriter,
                     directory: &mut DirectoryWriter,
                     kind: EntryKind,
                     role: u8,
                     id: u64|
     -> Result<()> {
        let len = writer.output().len();
        let mut entry_buf = [0u8; 40];
        let n = directory.push(
            Entry {
                kind,
                role,
                id,
                record_len: len as u64,
            },
            &mut entry_buf,
        )?;
        dir_entries.extend_from_slice(&entry_buf[..n]);
        new_objects.extend_from_slice(writer.output());
        writer.consume_output(len)?;
        Ok(())
    };

    let mut at = 0usize;
    let mut ignorable = 0u64;
    while at < objects.len() {
        match reader.read(&objects[at..])? {
            TileEvent::Begin { extent, exact_plan } => {
                let tile_id = extent.tile_id.ok_or("missing V4 tile id")?;
                writer.begin(exact_plan)?;
                flush(
                    &mut writer,
                    &mut directory,
                    EntryKind::TileBegin,
                    0,
                    tile_id,
                )?;
                at += extent.len as usize;
            }
            TileEvent::Object { extent, object } => {
                let (kind, role, id) = if object.kind & 1 == 0 {
                    let id = ignorable;
                    ignorable += 1;
                    (EntryKind::Ignorable, 0u8, id)
                } else if object.kind == MODEL_DEFINITION {
                    let mut bp = 0usize;
                    let role = u8::try_from(read_varint(object.body, &mut bp)?)?;
                    let id = read_varint(object.body, &mut bp)?;
                    let lifetime = read_varint(object.body, &mut bp)?;
                    let ek = match lifetime {
                        0 => EntryKind::Model,
                        1 => EntryKind::NextModel,
                        _ => return Err(format!("unknown model lifetime {lifetime}").into()),
                    };
                    (ek, role, id)
                } else {
                    let role = match object.kind {
                        UNIT_DC => 0,
                        UNIT_AC => 1,
                        UNIT_ALPHA => 2,
                        UNIT_EXACT => 3,
                        other => return Err(format!("unexpected V4 object kind {other}").into()),
                    };
                    let mut bp = 0usize;
                    let id = read_varint(object.body, &mut bp)?;
                    (EntryKind::Unit, role, id)
                };
                writer.append(object.kind, object.body)?;
                flush(&mut writer, &mut directory, kind, role, id)?;
                at += extent.len as usize;
            }
            TileEvent::End { extent, tile, .. } => {
                writer.end()?;
                flush(
                    &mut writer,
                    &mut directory,
                    EntryKind::TileEnd,
                    0,
                    tile.tile_id,
                )?;
                at += extent.len as usize;
            }
        }
    }
    reader.finish()?;
    writer.finish()?;
    let (count, objects_len) = directory.finish()?;
    if objects_len as usize != new_objects.len() {
        return Err("repacked object stream length mismatch".into());
    }

    // Build directory body
    let mut dir_body = Vec::with_capacity(dir_entries.len() + 18);
    put_varint(&mut dir_body, count);
    dir_body.extend_from_slice(&dir_entries);
    dir_body.extend_from_slice(&objects_len.to_le_bytes());

    // Build new DATA body preserving original directory placement (Front/Tail/None)
    let mut new_data = Vec::with_capacity(data.len() + 64);
    put_varint(&mut new_data, new_setup.len() as u64);
    new_data.extend_from_slice(&new_setup);
    let mut tail_uidx: Option<Vec<u8>> = None;
    match dir_prefix.placement {
        DirectoryPlacement::Front => {
            let needed_cap = (varint_len(dir_body.len() as u64) + dir_body.len()) as u64;
            let cap = dir_prefix.reserved_bytes.max(needed_cap);
            append_directory_prefix(
                &mut new_data,
                DirectoryPlacement::Front,
                cap,
                Some(&dir_body),
                MAX_TILE as u64,
            )?;
        }
        DirectoryPlacement::Tail => {
            append_directory_prefix(
                &mut new_data,
                DirectoryPlacement::Tail,
                0,
                None,
                MAX_TILE as u64,
            )?;
            let mut idx = dir_body;
            let payload_len = idx.len() + 12;
            let chunk_len = 4 + varint_len(payload_len as u64 + 1) + payload_len;
            idx.extend_from_slice(&(chunk_len as u64).to_le_bytes());
            idx.extend_from_slice(b"uIDX");
            tail_uidx = Some(idx);
        }
        DirectoryPlacement::None => {
            append_directory_prefix(
                &mut new_data,
                DirectoryPlacement::None,
                0,
                None,
                MAX_TILE as u64,
            )?;
        }
        other => return Err(format!("unsupported directory placement {other:?}").into()),
    }
    new_data.extend_from_slice(&new_objects);

    // Reassemble outer container without pSUM first (with fresh cSUM if original had cSUM)
    let mut head = Vec::with_capacity(header_end + 64);
    head.extend_from_slice(&input[..header_end]);
    for (tag, body) in &other_before_data {
        push_chunk(&mut head, tag, body);
    }
    let mut tail = Vec::with_capacity(new_data.len() + 64);
    push_chunk(&mut tail, b"DATA", &new_data);
    if let Some(idx) = &tail_uidx {
        push_chunk(&mut tail, b"uIDX", idx);
    }
    for (tag, body) in &other_after_data {
        push_chunk(&mut tail, tag, body);
    }

    let mut assembled = Vec::with_capacity(head.len() + tail.len() + 64);
    assembled.extend_from_slice(&head);
    if let Some((algo, width, k)) = csum_params {
        let csum = build_csum_chunk(&head, &tail, algo, width, k);
        assembled.extend_from_slice(&csum);
    }
    assembled.extend_from_slice(&tail);

    // Recompute pSUM (which also updates cSUM cleanly via imij::add_pixel_checksum!)
    let final_bytes = if had_psum {
        imij::add_pixel_checksum(assembled)?
    } else {
        assembled
    };

    // Strict verification: full decode verifies cSUM, ObjectIntegrity, AND pSUM with u8_dsqrt_d4
    let (dec_info, _) = imij::decode(&final_bytes)?;
    if dec_info != hdr.info {
        return Err("repacked tile decode geometry mismatch".into());
    }
    // Verify bit 5 is set in the final bytes
    let check_setup_ok = {
        let mut c = header_end;
        let mut found = false;
        while c < final_bytes.len() {
            let tag: [u8; 4] = final_bytes[c..c + 4].try_into().unwrap();
            c += 4;
            let l = read_varint(&final_bytes, &mut c)?;
            let b = if l == 0 {
                let s = &final_bytes[c..];
                c = final_bytes.len();
                s
            } else {
                let n = usize::try_from(l - 1)?;
                let s = &final_bytes[c..c + n];
                c += n;
                s
            };
            if &tag == b"DATA" {
                let mut p = 0;
                let sl = usize::try_from(read_varint(b, &mut p)?)?;
                let setup = &b[p..p + sl];
                let mut q = 0;
                for _ in 0..7 {
                    read_varint(setup, &mut q)?;
                }
                let _ql = read_varint(setup, &mut q)?;
                found = setup[q] & QUANTIZER_BIT_POWER != 0;
                break;
            }
        }
        found
    };
    if !check_setup_ok {
        return Err("verification failed: quantizer bit 5 not set in output".into());
    }
    Ok(final_bytes)
}

fn write_offsets(archive_path: &Path, output: &Path) -> Result<()> {
    let mut archive = Reader::open(SeekReader::new(File::open(archive_path)?))?;
    let entry = archive.get(0)?.ok_or("missing pyramid catalog")?;
    let catalog = PyramidCatalog::decode(&archive.read_entry_bounded(&entry, 64 << 10)?)?;
    let [level] = catalog.levels() else {
        return Err("expected one Explorer archive level".into());
    };
    if level.order != TileOrder::RowMajor
        || level.key_base != 1
        || (level.tile_width, level.tile_height) != (TW, TH)
    {
        return Err("expected row-major Explorer tile layout".into());
    }
    let count = u32::try_from(level.tile_count())?;
    let mut entries = archive.listing_from(1)?;
    let mut out = BufWriter::new(fresh(output)?);
    out.write_all(b"IMITILE2")?;
    for field in [level.width, level.height, TW, TH, count] {
        out.write_all(&field.to_le_bytes())?;
    }
    let mut end = None;
    let mut base = 0;
    for ordinal in 0..count {
        let entry = entries.next(&mut archive)?.ok_or("missing output tile")?;
        if entry.key() != u64::from(ordinal) + 1
            || end.is_some_and(|e| e != entry.offset())
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

fn repack_tiles_archive(source: &Path, output: &Path) -> Result<()> {
    let mut src = Reader::open(SeekReader::new(BufReader::new(File::open(source)?)))?;
    let head = src.get(0)?.ok_or("missing pyramid catalog")?;
    let cat_bytes = src.read_entry_bounded(&head, 64 << 10)?;
    let catalog = PyramidCatalog::decode(&cat_bytes)?;
    let [level] = catalog.levels() else {
        return Err("expected single-level tile archive".into());
    };
    let (width, height) = (level.width, level.height);
    let cols = width.div_ceil(TW);
    let count = u32::try_from(level.tile_count())?;

    let new_catalog = PyramidCatalog::new(vec![PyramidLevel::new(
        0,
        width,
        height,
        TW,
        TH,
        TileOrder::RowMajor,
        1,
    )?])?
    .encode();

    let mut entries = src.listing_from(1)?;
    let mut dst = Writer::new(BufWriter::new(fresh(output)?))
        .with_index_placement(IndexPlacement::AtCommit)?;
    dst.insert(
        0,
        new_catalog.len() as u64,
        0,
        Summary::default(),
        &mut new_catalog.as_slice(),
    )?;

    for start in (0..count).step_by(64) {
        let end = count.min(start + 64);
        let batch: Vec<_> = (start..end)
            .map(|i| -> Result<_> {
                let entry = entries.next(&mut src)?.ok_or("missing source tile")?;
                if entry.key() != u64::from(i) + 1 {
                    return Err("unexpected source tile key".into());
                }
                let w = (width - i % cols * TW).min(TW);
                let h = (height - i / cols * TH).min(TH);
                let payload = src.read_entry_bounded(&entry, MAX_TILE)?;
                Ok((i, w, h, payload))
            })
            .collect::<Result<_>>()?;

        let repacked: Vec<_> = batch
            .into_par_iter()
            .map(|(i, w, h, payload)| -> Result<_> {
                let out = repack_v4_lossy_tile(&payload)?;
                Ok((i, w, h, out))
            })
            .collect::<Result<_>>()?;

        for (i, w, h, encoded) in repacked {
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
    }
    if entries.next(&mut src)?.is_some() {
        return Err("extra source tiles".into());
    }
    dst.finish()?;
    write_offsets(output, &output.with_extension("index"))?;
    eprintln!(
        "repacked {count} V4 tiles ({width}x{height}) -> {}",
        output.display()
    );
    Ok(())
}

struct HashWriter {
    output: BufWriter<File>,
    hash: Hasher,
}

impl Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.output.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
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

fn opening_ranges(path: &Path, levels: &[(u32, u32, u32)]) -> Result<String> {
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
    for &(divisor, width, height) in levels
        .iter()
        .rev()
        .take_while(|&&(_, w, h)| w.max(h) <= 4096)
    {
        let id = u64::from(divisor);
        reader.view_info(id)?;
        pages.extend(reader.view_region_pages(id, Rect::new(0, 0, width, height))?);
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

/// Repack a multi-level native V4 pyramid (`milkyway/webmap`) in encoded domain.
fn repack_native_pyramid(source: &Path, output_dir: &Path) -> Result<()> {
    let reader = PlaneSetReader::open(
        SeekReader::new(File::open(source)?),
        PlaneSetLimits::default()
            .with_plane_pixels(1 << 40)
            .with_file_bytes(1 << 46),
    )?;
    let primary_len = reader.primary_len();
    let plane_descs = reader.planes().to_vec();
    if plane_descs.len() != 3 {
        return Err(format!("expected 3 color planes, got {}", plane_descs.len()).into());
    }
    let level_descs = plane_descs[0].levels().to_vec();
    let full_w = level_descs[0].width();
    let full_h = level_descs[0].height();

    // Read primary bytes verbatim (lossless thumbnail)
    let mut in_file = BufReader::with_capacity(4 << 20, File::open(source)?);
    let mut primary = vec![0u8; usize::try_from(primary_len)?];
    in_file.read_exact(&mut primary)?;
    let prim_hdr = imij::header(&primary)?;
    // If primary is itself lossy sRGB8 XYB, repack it too; otherwise keep lossless verbatim
    if prim_hdr.coding == Coding::Lossy
        && prim_hdr.lossy_color == Some(LossyColor::Xyb)
        && prim_hdr.info.sample == SampleType::U8
    {
        primary = repack_v4_lossy_tile(&primary)?;
    }

    // Reconstruct full level list including primary thumbnail
    let mut levels: Vec<(u32, u32, u32)> = Vec::new();
    let mut div = 1u32;
    for ld in &level_descs {
        levels.push((div, ld.width(), ld.height()));
        div = div.checked_mul(2).ok_or("divisor overflow")?;
    }
    levels.push((div, prim_hdr.info.width, prim_hdr.info.height));
    let thumb_div = div;

    let mut total_records = 0u64;
    for ld in &level_descs {
        total_records += u64::from(ld.width().div_ceil(ld.tile_width()))
            * u64::from(ld.height().div_ceil(ld.tile_height()));
    }

    std::fs::create_dir(output_dir)?;
    let output_path = output_dir.join("pyramid.imi.partial");
    let mut writer = PlaneSetWriter::new(
        &primary,
        HashWriter {
            output: BufWriter::with_capacity(4 << 20, fresh(&output_path)?),
            hash: Hasher::new(HashAlgorithm::Xxh3_128),
        },
        fresh(&output_dir.join("directory.scratch"))?,
        PlaneSetLimits::default()
            .with_plane_pixels(1 << 40)
            .with_file_bytes(1 << 46),
        match total_records {
            0..=256 => 64,
            257..=4096 => 256,
            _ => 1024,
        },
        total_records.checked_mul(10).ok_or("index scratch size")?,
    )?;

    let mut planes = Vec::with_capacity(3);
    for name in ["red", "green", "blue"] {
        planes.push(writer.add_plane(PlaneDescriptor::new(name, SampleType::U8))?);
    }

    let mut views = Vec::new();
    for &(divisor, w, h) in &levels {
        let id = u64::from(divisor);
        let label = if id == 1 {
            "full".to_owned()
        } else {
            format!("level-{id}")
        };
        let backing = if divisor == thumb_div {
            ViewBacking::Primary
        } else {
            let ordinal = writer.declare_color_level(
                &planes,
                LevelDescriptor::new(w, h, TW, TH),
                Channels::Rgb,
                Coding::Lossy,
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
            view = view.with_registration(ViewRegistration::resized(full_w, full_h, w, h)?);
        }
        views.push(view);
    }
    writer.set_image_directory(ImageDirectory::new(1, views)?)?;

    // Stream pREC records sequentially starting right after primary
    in_file.seek(SeekFrom::Start(primary_len))?;
    let mut next_expected_rid = 0u64;
    let mut done_records = 0u64;

    let read_next_prec = |in_file: &mut BufReader<File>, expected_rid: u64| -> Result<Vec<u8>> {
        loop {
            let mut tag = [0u8; 4];
            in_file.read_exact(&mut tag)?;
            let len_plus_one = read_varint_reader(in_file)?;
            let body_len = usize::try_from(len_plus_one.checked_sub(1).ok_or("unbounded pREC")?)?;
            if &tag == b"pREC" {
                let mut body = vec![0u8; body_len];
                in_file.read_exact(&mut body)?;
                let mut bp = 0usize;
                let rid = read_varint(&body, &mut bp)?;
                if rid != expected_rid {
                    return Err(format!(
                        "unexpected pREC record id {rid}, expected {expected_rid}"
                    )
                    .into());
                }
                return Ok(body[bp..].to_vec());
            } else {
                // Skip non-pREC chunks (shouldn't happen before pIDX, but safe)
                in_file.seek(SeekFrom::Current(body_len as i64))?;
            }
        }
    };

    for (ordinal, ld) in level_descs.iter().enumerate() {
        let mut records = writer.record_range(planes[0], u32::try_from(ordinal)?)?;
        let level_tiles = u64::from(ld.width().div_ceil(ld.tile_width()))
            * u64::from(ld.height().div_ceil(ld.tile_height()));
        let mut remaining = level_tiles;
        while remaining > 0 {
            let batch_n = remaining.min(64) as usize;
            let mut batch = Vec::with_capacity(batch_n);
            for _ in 0..batch_n {
                let rec = records.next().ok_or("record range exhausted")?;
                let raw = read_next_prec(&mut in_file, next_expected_rid)?;
                next_expected_rid += 1;
                batch.push((rec, raw));
            }
            let repacked: Vec<_> = batch
                .into_par_iter()
                .map(|(rec, raw)| -> Result<_> {
                    let out = repack_v4_lossy_tile(&raw)?;
                    Ok((rec, out))
                })
                .collect::<Result<_>>()?;
            for (rec, out) in repacked {
                writer.write_tile(rec, &out)?;
            }
            remaining -= batch_n as u64;
            done_records += batch_n as u64;
            if done_records % 4096 < 64 || done_records == total_records {
                eprintln!(
                    "native pyramid: {done_records} / {total_records} tiles ({:.1}%)",
                    done_records as f64 * 100.0 / total_records as f64
                );
            }
        }
        if records.next().is_some() {
            return Err("extra tile record in level".into());
        }
    }

    let (hw, scratch) = writer.finish()?;
    let bytes = hw.output.get_ref().metadata()?.len();
    let digest = format!("{:x}", hw.hash.finish());
    drop(hw.output);
    drop(scratch);
    let final_path = output_dir.join("pyramid.imi");
    std::fs::rename(&output_path, &final_path)?;
    std::fs::remove_file(output_dir.join("directory.scratch"))?;
    let opening = opening_ranges(&final_path, &levels)?;
    let rows: Vec<_> = levels
        .iter()
        .map(|&(d, w, h)| {
            format!("{{\"id\":\"{d}\",\"divisor\":{d},\"width\":{w},\"height\":{h}}}")
        })
        .collect();
    writeln!(
        fresh(&output_dir.join("pyramid.json"))?,
        "{{\"contentId\":\"{digest}\",\"encodedBytes\":{bytes},\"qualityTarget\":70,\"opening\":[{opening}],\"views\":[{}]}}",
        rows.join(",")
    )?;
    eprintln!(
        "repacked native pyramid ({total_records} tiles, {bytes} bytes, xxh3={digest}) -> {}",
        final_path.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, src, dst] if mode == "tiles" => repack_tiles_archive(Path::new(src), Path::new(dst)),
        [mode, src, dst] if mode == "native" => repack_native_pyramid(Path::new(src), Path::new(dst)),
        [mode, src, dst] if mode == "single" => {
            let data = std::fs::read(src)?;
            let out = repack_v4_lossy_tile(&data)?;
            std::fs::write(dst, out)?;
            Ok(())
        }
        _ => Err(
            "usage:\n  repack_xyb_power tiles IN.imijset OUT.imijset\n  repack_xyb_power native IN.imi OUT_DIR\n  repack_xyb_power single IN.imi OUT.imi"
                .into(),
        ),
    }
}
