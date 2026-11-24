//! Reader admission: group records and stripe state grow only as input
//! actually arrives, never on head-declared geometry alone.
// Literal V2/V3 framing controls use the retained compatibility writers.
#![cfg(feature = "std")]
mod common;

use std::io::Cursor;

use imij::{Channels, Error, ImageInfo, Limits, Metadata, SampleType, decode, decode_reader};

/// `DATA` chunk payload bounds `[payload_start, file_end)` in file bytes.
fn data_range(file: &[u8]) -> (usize, usize) {
    let mut at = common::header_end(file);
    assert_eq!(&file[at..at + 4], b"DATA");
    at += 4;
    common::read_varint(file, &mut at); // declared length; payload follows the varint
    (at, file.len())
}

/// Lossless head length inside the DATA payload of a gray u8 image (SPEC
/// 2: the head, its records included).
fn head_len(d: &[u8]) -> usize {
    let gray = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    common::payload_head(d, &gray, false).head_end
}

fn put_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(tag);
    if data.is_empty() {
        out.push(0);
    } else {
        out.extend_from_slice(&common::varint_bytes(data.len() + 1));
        out.extend_from_slice(data);
    }
}

/// Offset of the width varint inside the file: after the fields the
/// header's `present` mask marks (SPEC 1.0).
fn width_at(file: &[u8]) -> usize {
    let r = common::header_record(file);
    r.start + common::header_layout(&file[r]).dims
}

/// `enc` re-spliced to width `new_w`, with the header-record length
/// recomputed.
fn splice_width(enc: &[u8], new_w: usize) -> Vec<u8> {
    let (rs, re) = {
        let r = common::header_record(enc);
        (r.start, r.end)
    };
    let mut at = width_at(enc);
    let mut hdr = enc[rs..at].to_vec();
    hdr.extend_from_slice(&common::varint_bytes(new_w));
    common::read_varint(enc, &mut at); // original width
    hdr.extend_from_slice(&enc[at..re]);
    let mut out = enc[..8].to_vec();
    out.extend_from_slice(&common::varint_bytes(hdr.len()));
    out.extend_from_slice(&hdr);
    out.extend_from_slice(&enc[re..]);
    out
}

/// `enc` (no metadata) with its DATA payload rewritten without the unit
/// table hint (SPEC 2.7): the layout of a writer that cannot buffer its
/// records, whose record reading these tests exercise.
fn length_prefixed(enc: &[u8], lossy: bool) -> Vec<u8> {
    let info = imij::header(enc).unwrap().info;
    let (ds, de) = data_range(enc);
    let d = &enc[ds..de];
    let payload = common::without_hint(d, &common::payload(d, &info, lossy));
    let mut out = enc[..common::header_end(enc)].to_vec();
    put_chunk(&mut out, b"DATA", &payload);
    out
}

/// Encoded grayscale image re-spliced to `new_w`, with `DATA` emitted
/// bounded (default) or open (len=0), in the length-prefixed layout.
fn grouped_file(new_w: usize, open_data: bool) -> Vec<u8> {
    grouped(new_w, open_data, false)
}

/// [`grouped_file`] in either layout (`table`: as the encoder writes it).
fn grouped(new_w: usize, open_data: bool, table: bool) -> Vec<u8> {
    let px: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let enc = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_column_groups(Some(1));
        cfg
    }
    .encode_legacy_samples(
        imij::ImageRef::new(&px, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap();
    let enc = if table {
        enc
    } else {
        length_prefixed(&enc, false)
    };
    let out = splice_width(&enc, new_w);
    if !open_data {
        return out;
    }
    let (ds, de) = data_range(&out);
    let mut open = Vec::new();
    let at = common::header_end(&out);
    open.extend_from_slice(&out[..at]);
    open.extend_from_slice(b"DATA");
    open.push(0);
    open.extend_from_slice(&out[ds..de]);
    open
}

/// Lossy head length inside the DATA payload of an RGB(A) u8 image (SPEC
/// 2.8: parameters, tables, head records and the nested heads).
fn lossy_head_len(d: &[u8], alpha: bool) -> usize {
    let ch = if alpha { Channels::Rgba } else { Channels::Rgb };
    let info = ImageInfo::new(1, 1, ch, SampleType::U8);
    common::payload_head(d, &info, true).head_end
}

/// Encoded lossy image re-spliced to `new_w`, DATA kept bounded, in the
/// length-prefixed layout.
fn grouped_lossy_file(new_w: usize, open_data: bool) -> Vec<u8> {
    grouped_lossy(new_w, open_data, false)
}

/// [`grouped_lossy_file`] in either layout.
fn grouped_lossy(new_w: usize, open_data: bool, table: bool) -> Vec<u8> {
    let px: Vec<u8> = (0..64 * 8 * 3).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(64, 8, Channels::Rgb, SampleType::U8);
    let mut cfg = imij::ImiEncoderConfig::lossy(70.0);
    cfg = cfg.with_column_groups(Some(8));
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let enc = if table {
        enc
    } else {
        length_prefixed(&enc, true)
    };
    let out = splice_width(&enc, new_w);
    if !open_data {
        return out;
    }
    let (ds, de) = data_range(&out);
    let at = common::header_end(&out);
    let mut open = out[..at].to_vec();
    open.extend_from_slice(b"DATA");
    open.push(0);
    open.extend_from_slice(&out[ds..de]);
    open
}

fn rows(file: &[u8]) -> Result<Vec<u8>, Error> {
    rows_limited(file, &Limits::DEFAULT)
}

fn rows_limited(file: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut got = Vec::new();
    let mut rd = Cursor::new(file);
    decode_reader(&mut rd, limits, |_, _, row| {
        got.extend_from_slice(row);
        Ok(())
    })?;
    Ok(got)
}

/// Feeds one byte per `read` — every internal read can see a short buffer.
struct Trickle<'a> {
    data: &'a [u8],
}

impl<'a> std::io::Read for Trickle<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.data.is_empty() {
            return Ok(0);
        }
        buf[0] = self.data[0];
        self.data = &self.data[1..];
        Ok(1)
    }
}

#[test]
fn reader_rejects_record_count_beyond_bounded_data() {
    // A bounded DATA that cannot hold `ngroups` one-byte-minimum records
    // fails closed before a single record byte is read, with the slice
    // parser's `stripe count exceeds data`, and the cursor never advances
    // past the head.
    let file = grouped_file(2 << 20, false);
    let (ds, _) = data_range(&file);
    let expect_pos = ds + head_len(&file[ds..]);
    let mut rd = Cursor::new(file);
    let err = decode_reader(&mut rd, &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::Corrupt("stripe count exceeds data"));
    assert_eq!(rd.position(), expect_pos as u64);
}

#[test]
fn reader_open_data_records_fail_on_truncation() {
    // Open DATA has no declared bound, so growth tracks delivered input: a
    // ~200-byte file can only ever deliver ~200 records, and the decoder
    // stops on the first truncated record at end of input rather than
    // reserving head-declared group state.
    let file = grouped_file(2 << 20, true);
    let len = file.len() as u64;
    let mut rd = Cursor::new(file);
    let err = decode_reader(&mut rd, &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::Corrupt("truncated file"));
    assert_eq!(rd.position(), len);
}

/// Yields bytes until `fail`, then reports an I/O error — a mid-DATA
/// transport failure, not a truncated file.
struct FailAt<'a> {
    data: &'a [u8],
    at: usize,
    fail: usize,
}

impl<'a> std::io::Read for FailAt<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.fail {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "boom"));
        }
        if buf.is_empty() || self.at >= self.data.len() {
            return Ok(0);
        }
        buf[0] = self.data[self.at];
        self.at += 1;
        Ok(1)
    }
}

#[test]
fn reader_mid_data_io_error_stays_read_error() {
    // A transport failure inside DATA must surface as `Error::Read`, not be
    // flattened into `Corrupt` — callers distinguish a bad file from a bad
    // source. This covers the open-DATA delivered-input read path.
    let file = grouped_file(256, true);
    let (ds, _) = data_range(&file);
    let fail = ds + head_len(&file[ds..]) + 2; // two bytes into the records
    let err = decode_reader(
        FailAt {
            data: &file,
            at: 0,
            fail,
        },
        &Limits::DEFAULT,
        |_, _, _| Ok(()),
    )
    .err()
    .unwrap();
    assert_eq!(err, Error::Read);
}

#[test]
fn reader_bounded_data_trailing_garbage_rejected() {
    // Records consumed ≠ declared DATA length must fail closed:
    // `src.pos - start != n - 1` is `Corrupt`, not silently skipped.
    let px: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (ds, de) = data_range(&enc);
    let at = common::header_end(&enc);
    let mut out = enc[..at].to_vec();
    out.extend_from_slice(b"DATA");
    out.extend_from_slice(&common::varint_bytes(de - ds + 1 + 7)); // declared n = payload + 1
    out.extend_from_slice(&enc[ds..de]);
    out.extend_from_slice(&[0u8; 7]);
    let err = decode_reader(&mut Cursor::new(&out), &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::Corrupt("trailing DATA bytes"));
}

#[test]
fn reader_huge_declared_data_len_fails_closed() {
    // The DATA length varint has no `max_bytes` gate; a u64-adjacent
    // declared length must fail `TooLarge` via checked arithmetic, not wrap
    // `src.end`.
    let px: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (ds, de) = data_range(&enc);
    let at = common::header_end(&enc);
    let mut out = enc[..at].to_vec();
    out.extend_from_slice(b"DATA");
    out.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]); // u64::MAX
    out.extend_from_slice(&enc[ds..de]);
    let err = decode_reader(&mut Cursor::new(&out), &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::TooLarge);
}

#[test]
fn reader_zero_length_records_fail_cleanly() {
    // Zero-length records are structurally valid at the record layer; the
    // reader must reject inside `StripeState::new_with` without committing
    // group state for the remaining `ngroups - seen` units.
    let px: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(64, 64, Channels::Gray, SampleType::U8);
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (_, re) = {
        let r = common::header_record(&enc);
        (r.start, r.end)
    };
    // Without the hint, whose spans would not match the forged records.
    let (ds, _) = data_range(&enc);
    let plain = match common::payload(&enc[ds..], &info, false).table() {
        Some(_) => length_prefixed(&enc, false),
        None => enc.clone(),
    };
    let (ds, _) = data_range(&plain);
    let mut payload = plain[ds..ds + head_len(&plain[ds..])].to_vec();
    for _ in 0..64 {
        payload.extend_from_slice(&[59, 0]); // an empty stripe record
    }
    let mut out = enc[..re].to_vec();
    put_chunk(&mut out, b"DATA", &payload);
    assert_eq!(rows(&out).err().unwrap(), Error::Corrupt("truncated"));
}

#[test]
fn reader_extended_header_decodes() {
    // Headers may carry ignorable extension records (even ids). The reader
    // contract is the caller's `Limits`, not the session's 64 KiB
    // `DecodePolicy::header` default: a >64 KiB header must still decode,
    // identically to `decode` — including under `Limits::NONE`.
    let px: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (rs, re) = {
        let r = common::header_record(&enc);
        (r.start, r.end)
    };
    let mut hdr = enc[rs..re].to_vec();
    hdr.push(6); // ignorable extension id (unassigned; 2 is the file checksum, 4 the DATA locator)
    hdr.extend_from_slice(&common::varint_bytes(80_000));
    hdr.resize(hdr.len() + 80_000, 0);
    let mut out = enc[..8].to_vec();
    out.extend_from_slice(&common::varint_bytes(hdr.len()));
    out.extend_from_slice(&hdr);
    out.extend_from_slice(&enc[re..]);
    let (_, want) = decode(&out).unwrap();
    assert_eq!(rows(&out).unwrap(), want);
    assert_eq!(rows_limited(&out, &Limits::NONE).unwrap(), want);
}

#[test]
fn reader_grouped_multi_stripe_decodes_identically() {
    // Valid grouped multi-stripe streams keep their exact row order; group
    // state and record buffers are reused across stripes.
    let (w, h) = (20_000u32, 16u32);
    let px: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(w, h, Channels::Gray, SampleType::U8);
    let enc = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(2);
        });
        cfg = cfg.with_column_groups(Some(4));
        cfg
    }
    .encode_legacy_samples(
        imij::ImageRef::new(&px, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap();
    let (_, want) = decode(&enc).unwrap();
    assert_eq!(rows(&enc).unwrap(), want);
}

#[test]
fn reader_lossy_open_data_records_fail_on_truncation() {
    // Open-DATA lossy: growth tracks delivered input; a spliced-wide head
    // demands `ng × stripes` interleaved records the file cannot contain.
    let file = grouped_lossy_file(3_000_000, true);
    let len = file.len() as u64;
    let mut rd = Cursor::new(file);
    let err = decode_reader(&mut rd, &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::Corrupt("truncated file"));
    assert_eq!(rd.position(), len);
}

#[test]
fn reader_record_crossing_data_boundary_rejected() {
    // A record's declared length that crosses the bounded-DATA end fails
    // before the payload is touched: `Corrupt("record past end of DATA")`
    // (every route's message) at the position right after the length
    // varint.
    let file = grouped_file(16, false);
    let (ds, de) = data_range(&file);
    // The first record's length varint (after its id).
    let at0 = ds + head_len(&file[ds..]) + 1;
    let mut at = at0;
    common::read_varint(&file, &mut at); // original record length
    let nv = common::varint_bytes(de - at0); // one byte more than fits, wherever it lands
    let mut out = file[..at0].to_vec();
    out.extend_from_slice(&nv);
    out.extend_from_slice(&file[at..]);
    let mut rd = Cursor::new(out);
    let err = decode_reader(&mut rd, &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::Corrupt("record past end of DATA"));
    assert_eq!(rd.position(), (at0 + nv.len()) as u64);
}

#[test]
fn reader_bounded_data_huge_record_early_eof_reserves_nothing() {
    // A bounded DATA length is as attacker-declared as a record length: a
    // 1 TiB DATA holding a 512 GiB first record, followed by end of input,
    // must fail as a truncated file. Reserving the declared record before
    // its bytes arrive would instead surface as `TooLarge` (or exhaust
    // memory under overcommit).
    let file = grouped_file(16, false);
    let (ds, _) = data_range(&file);
    let tag = common::header_end(&file);
    // The first record's length varint (after its id).
    let at0 = ds + head_len(&file[ds..]) + 1;
    let mut at = at0;
    common::read_varint(&file, &mut at);
    let mut out = file[..tag + 4].to_vec();
    out.extend_from_slice(&common::varint_bytes(1u64 << 40));
    out.extend_from_slice(&file[ds..at0]);
    out.extend_from_slice(&common::varint_bytes(1u64 << 39));
    out.extend_from_slice(&file[at..]);
    let len = out.len() as u64;
    let mut rd = Cursor::new(out);
    let err = decode_reader(&mut rd, &Limits::NONE, |_, _, _| Ok(()))
        .err()
        .unwrap();
    if usize::BITS >= 64 {
        assert_eq!(err, Error::Corrupt("truncated file"));
        assert_eq!(rd.position(), len);
    } else {
        // These lengths can't be addressed: refused before reading the body.
        assert_eq!(err, Error::TooLarge);
        assert!(rd.position() < len);
    }
}

#[test]
fn reader_lossy_rejects_record_count_beyond_bounded_data() {
    // Same contract as the lossless reader: `ng * stripes` records minimum
    // cannot fit a bounded DATA with fewer bytes left, so the decoder must
    // reject before committing group state or stripe buffers — here the
    // cursor never advances past the lossy head.
    let file = grouped_lossy_file(3 << 20, false); // ng = 3M/8 groups, ~1 KiB payload
    let (ds, _) = data_range(&file);
    let expect_pos = ds + lossy_head_len(&file[ds..], false);
    let mut rd = Cursor::new(file);
    let err = decode_reader(&mut rd, &Limits::DEFAULT, |_, _, _| Ok(()))
        .err()
        .unwrap();
    assert_eq!(err, Error::Corrupt("stripe count exceeds data"));
    assert_eq!(rd.position(), expect_pos as u64);
}

#[test]
fn reader_lossy_grouped_multi_stripe_decodes_identically() {
    // Valid grouped multi-stripe lossy streams keep exact row order and
    // reuse group state, stripe buffers and record buffers across stripes.
    let (w, h) = (256u32, 48u32);
    let px: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut cfg = imij::ImiEncoderConfig::lossy(70.0);
    cfg = cfg.with_column_groups(Some(64));
    cfg = cfg.tune_lossy(|t| {
        t.stripe_block_rows = 1;
    });
    let enc = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (_, want) = decode(&enc).unwrap();
    assert_eq!(rows(&enc).unwrap(), want);
    // One byte at a time: identical output through a trickling reader.
    let mut got = Vec::new();
    decode_reader(Trickle { data: &enc }, &Limits::DEFAULT, |_, _, row| {
        got.extend_from_slice(row);
        Ok(())
    })
    .unwrap();
    assert_eq!(got, want);
}

#[test]
fn reader_lossy_alpha_decodes_identically() {
    // Alpha lossy streams exercise the `a_row`/`alpha_px`/`out_row` lazy
    // allocations and `pack_rows`; both alpha channel kinds agree with the
    // in-memory decoder.
    for ch in [Channels::Rgba, Channels::GrayAlpha] {
        let (w, h) = (128u32, 32u32);
        let bpp = ch.count();
        let px: Vec<u8> = (0..w as usize * h as usize * bpp)
            .map(|i| (i * 17 % 251) as u8)
            .collect();
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0);
        cfg = cfg.with_column_groups(Some(32));
        cfg = cfg.tune_lossy(|t| {
            t.stripe_block_rows = 1;
        });
        let enc = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let (_, want) = decode(&enc).unwrap();
        assert_eq!(rows(&enc).unwrap(), want, "{ch:?}");
    }
}

#[test]
fn reader_parity_matrix_sample_types_and_groups() {
    // Slice == reader across sample types and group layouts for lossless;
    // `stripe_rows: 2` forces multi-stripe reuse of records/state.
    for (ch, st) in [
        (Channels::Gray, SampleType::U8),
        (Channels::Rgb, SampleType::U8),
        (Channels::Rgba, SampleType::U8),
        (Channels::Gray, SampleType::U16),
        (Channels::GrayAlpha, SampleType::U16),
        (Channels::Rgb, SampleType::U16),
        (Channels::Rgb, SampleType::F16),
    ] {
        for groups in [None, Some(8)] {
            let info = ImageInfo::new(96, 24, ch, st);
            let rb = info.row_bytes();
            let px: Vec<u8> = (0..info.height as usize * rb)
                .map(|i| (i * 31 + 7) as u8)
                .collect();
            let enc = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(2);
                });
                cfg = cfg.with_column_groups(groups);
                cfg
            }
            .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
            .unwrap();
            let (_, want) = decode(&enc).unwrap();
            assert_eq!(rows(&enc).unwrap(), want, "{ch:?}/{st:?} groups={groups:?}");
        }
    }
}

#[test]
fn reader_padded_stride_parity() {
    // `stride > row_bytes`: padding must never appear in reader output.
    let info = ImageInfo::new(48, 16, Channels::Rgb, SampleType::U8);
    let rb = info.row_bytes();
    let stride = rb + 13;
    let mut px = vec![0xEEu8; stride * info.height as usize];
    for y in 0..info.height as usize {
        for x in 0..rb {
            px[y * stride + x] = (x + y * 7) as u8;
        }
    }
    let enc = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(3);
        });
        cfg = cfg.with_column_groups(Some(8));
        cfg
    }
    .encode_legacy_samples(
        imij::ImageRef::new(&px, stride, &info),
        &Metadata::default(),
    )
    .unwrap();
    let (_, want) = decode(&enc).unwrap();
    let tight: Vec<u8> = (0..info.height as usize)
        .flat_map(|y| px[y * stride..y * stride + rb].to_vec())
        .collect();
    assert_eq!(want, tight);
    assert_eq!(rows(&enc).unwrap(), want);
}

#[test]
fn reader_open_data_trailing_chunks_metadata() {
    // Open DATA followed by ancillary chunks: the reader keeps parsing
    // chunk records after the pixel payload and retains the metadata — the
    // same contract `decode`/`probe` uphold (container.rs covers those).
    let px: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(16, 16, Channels::Gray, SampleType::U8);
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (ds, de) = data_range(&enc);
    let at = common::header_end(&enc);
    let mut out = enc[..at].to_vec();
    out.extend_from_slice(b"DATA");
    out.push(0);
    out.extend_from_slice(&enc[ds..de]);
    put_chunk(&mut out, b"eXIf", b"exif");
    let mut got = Vec::new();
    let (ri, meta) = decode_reader(&mut Cursor::new(&out), &Limits::DEFAULT, |_, _, row| {
        got.extend_from_slice(row);
        Ok(())
    })
    .unwrap();
    assert_eq!(ri, info);
    assert_eq!(meta.exif.as_deref(), Some(&b"exif"[..]));
    let (_, want) = decode(&enc).unwrap();
    assert_eq!(got, want);
}

#[test]
fn reader_callback_error_aborts_decode() {
    // A row-callback error propagates unchanged and stops decoding — no
    // further rows are delivered after it.
    let px: Vec<u8> = (0..64 * 16).map(|i| (i % 251) as u8).collect();
    let info = ImageInfo::new(64, 16, Channels::Gray, SampleType::U8);
    let enc = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut n = 0u32;
    let err = decode_reader(&mut Cursor::new(&enc), &Limits::DEFAULT, |_, _, _| {
        n += 1;
        if n == 2 {
            Err(Error::InvalidInput("stop"))
        } else {
            Ok(())
        }
    })
    .err()
    .unwrap();
    assert_eq!(err, Error::InvalidInput("stop"));
    assert_eq!(n, 2);
}

/// The unit table hint against the record count: a spliced head that needs
/// millions of records with a 16-entry hint fails at the end of the head
/// (the head parser matches the hint to the geometry), before any record or
/// group state is reserved, bounded or open.
#[test]
fn reader_rejects_unit_table_shorter_than_its_units() {
    for open in [false, true] {
        for lossy in [false, true] {
            let file = if lossy {
                grouped_lossy(3 << 20, open, true)
            } else {
                grouped(2 << 20, open, true)
            };
            let (ds, _) = data_range(&file);
            let head = if lossy {
                lossy_head_len(&file[ds..], false)
            } else {
                head_len(&file[ds..])
            };
            let mut rd = Cursor::new(file);
            let err = decode_reader(&mut rd, &Limits::DEFAULT, |_, _, _| Ok(()))
                .err()
                .unwrap();
            let what = format!("open {open}, lossy {lossy}");
            assert_eq!(err, Error::Corrupt("unit table length"), "{what}");
            assert_eq!(rd.position(), (ds + head) as u64, "{what}");
        }
    }
}
