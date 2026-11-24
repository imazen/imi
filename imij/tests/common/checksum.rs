//! Fixtures of the `cSUM` tests (SPEC 1.4): files from every writer,
//! an independent table writer (xxhash-rust's XXH3) for any piece size,
//! width and position, and every read path with verification on and off.
// Each test file uses a different part of it.
#![allow(dead_code, unused_imports)]

pub use std::io::Read;

pub use imij::decode_resources::{DecodePolicy, DecodeSession};
pub use imij::planes::*;
pub use imij::{
    Channels, Chunk, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, GainMap,
    GainMapParams, ImageInfo, ImageRef, ImiEncoderConfig, Limits, Metadata, SampleType,
    StreamDecoder, decode, decode_into_threads, decode_into_threads_with_limits, decode_reader,
    decode_with_limits, header, previews, probe,
};

pub const MISMATCH: Error = Error::Corrupt("file checksum mismatch");
pub const OFF: Limits = Limits::DEFAULT.with_checksum_verification(false);

pub fn image(w: u32, h: u32, channels: Channels) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, channels, SampleType::U8);
    let px = (0..info.row_bytes() as u64 * u64::from(h))
        .map(|i| ((i * 7 + (i / 5) * 13 + (i * i) % 17) % 251) as u8)
        .collect();
    (info, px)
}

pub fn meta() -> Metadata {
    Metadata {
        icc: Some(b"icc profile bytes".repeat(3)),
        exif: Some(b"MM\0*exif".to_vec()),
        chunks: vec![Chunk {
            tag: *b"vnDr",
            data: b"opaque vendor payload".to_vec(),
        }],
        ..Default::default()
    }
}

pub fn lossless_sized(w: u32, h: u32, checksum: bool) -> Vec<u8> {
    let (info, px) = image(w, h, Channels::Rgb);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_checksum(checksum);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg
    };
    cfg.encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta())
        .unwrap()
}

pub fn lossless(checksum: bool) -> Vec<u8> {
    lossless_sized(13, 7, checksum)
}

pub fn lossy(checksum: bool) -> Vec<u8> {
    let (info, px) = image(24, 16, Channels::Rgba);
    let cfg = imij::ImiEncoderConfig::lossy(80.0).with_checksum(checksum);
    cfg.encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta())
        .unwrap()
}

// These V3 controls deliberately exercise alternate checksum placement and
// append chunks after DATA. V4 tail directories have their own conformance gates.

// ---- framing and an independent table writer --------------------------------

pub use super::put_varint;

/// The varint at `d[*at..]`; advances `at` past it.
pub fn get_varint(d: &[u8], at: &mut usize) -> u64 {
    super::read_varint(d, at)
}

pub fn varint_len(v: u64) -> usize {
    super::varint_bytes(v).len()
}

/// End of the header.
pub fn head_end(f: &[u8]) -> usize {
    super::header_end(f)
}

/// (offset, tag, payload range) of every chunk; an open chunk runs to the
/// end of the file.
pub fn chunks(f: &[u8]) -> Vec<(usize, [u8; 4], std::ops::Range<usize>)> {
    super::chunks(f)
        .into_iter()
        .map(|(tag, at, end)| (at, tag, at + 4 + super::varint(&f[at + 4..]).1..end))
        .collect()
}

pub fn find(f: &[u8], tag: &[u8; 4]) -> Option<(usize, std::ops::Range<usize>)> {
    chunks(f)
        .into_iter()
        .find(|c| &c.1 == tag)
        .map(|c| (c.0, c.2))
}

/// (start, end) of the whole cSUM chunk.
pub fn csum(f: &[u8]) -> (usize, usize) {
    let (at, r) = find(f, b"cSUM").expect("cSUM chunk");
    (at, r.end)
}

/// Offset of the header record's piece-size byte (header extension 2; the
/// DATA locator, extension 4, may follow it).
pub fn record_at(f: &[u8]) -> usize {
    let (_, r) = super::extension_ranges(f)
        .into_iter()
        .find(|e| e.0 == 2)
        .expect("header record");
    assert!(r.len() == 1 || r.len() == 2, "header record");
    r.start
}

/// The file without its cSUM chunk.
pub fn strip(f: &[u8]) -> Vec<u8> {
    let (a, b) = csum(f);
    [&f[..a], &f[b..]].concat()
}

/// `f` (without a cSUM chunk) without its header record.
pub fn drop_record(f: &[u8]) -> Vec<u8> {
    let recs: Vec<_> = super::extensions(f)
        .into_iter()
        .filter(|e| e.0 != 2)
        .collect();
    super::with_extensions(f, &recs)
}

/// The file without its cSUM chunk and header record.
pub fn strip_all(f: &[u8]) -> Vec<u8> {
    drop_record(&strip(f))
}

#[derive(Clone, Copy, Debug)]
pub enum Place {
    BeforeData,
    Last,
    /// Before the n-th chunk.
    Chunk(usize),
}

/// The cSUM chunk for covered bytes `cov` (reference XXH3, any k and width).
/// A `cSUM` chunk built independently of the library: algorithm 1 (XXH3-128)
/// with `width` low bytes per piece.
pub fn table_chunk(cov: &[u8], k: u8, width: u8) -> Vec<u8> {
    let mut payload = vec![1, width, k];
    put_varint(&mut payload, cov.len() as u64);
    // Pieces of 2^32 bytes or more do not fit a 32-bit usize: one piece.
    let size = usize::try_from(1u64 << k).unwrap_or(usize::MAX);
    for piece in cov.chunks(size) {
        let d = xxhash_rust::xxh3::xxh3_128(piece);
        payload.extend_from_slice(&d.to_le_bytes()[..width as usize]);
    }
    let mut c = b"cSUM".to_vec();
    put_varint(&mut c, payload.len() as u64 + 1);
    c.extend_from_slice(&payload);
    c
}

/// `base` (no cSUM chunk; with the header record) with a fresh table.
pub fn with_table(base: &[u8], k: u8, width: u8, place: Place) -> Vec<u8> {
    let mut cov = base.to_vec();
    let rec = record_at(&cov);
    cov[rec] = k;
    let chunk = table_chunk(&cov, k, width);
    let cs = chunks(&cov);
    let at = match place {
        Place::BeforeData => cs.iter().find(|c| &c.1 == b"DATA").unwrap().0,
        Place::Last => cov.len(),
        Place::Chunk(i) => cs.get(i).map_or(cov.len(), |c| c.0),
    };
    [&cov[..at], &chunk[..], &cov[at..]].concat()
}

/// The library's file, rebuilt with the reference writer at the same place.
pub fn rebuilt(f: &[u8]) -> Vec<u8> {
    let (a, _) = csum(f);
    let base = strip(f);
    let k = f[record_at(f)];
    let place = chunks(&base)
        .iter()
        .position(|c| c.0 == a)
        .map_or(Place::Last, Place::Chunk);
    with_table(&base, k, 16, place)
}

// ---- every read path --------------------------------------------------------

/// A reader delivering 1..=7 bytes per call.
pub struct Chunked<'a> {
    pub data: &'a [u8],
    pub calls: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.calls += 1;
        let n = (1 + self.calls % 7).min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

pub fn rows_into(info: &ImageInfo, out: &mut Vec<u8>, y: u32, row: &[u8]) -> Result<(), Error> {
    assert_eq!(y as usize * info.row_bytes(), out.len());
    out.extend_from_slice(row);
    Ok(())
}

pub fn via_reader_limits(r: impl Read, limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    decode_reader(r, limits, |i, y, row| rows_into(i, &mut out, y, row))?;
    Ok(out)
}

pub fn via_reader(r: impl Read) -> Result<Vec<u8>, Error> {
    via_reader_limits(r, &Limits::DEFAULT)
}

/// Output size from the (unverified) header, capped: changed-byte tests
/// must not allocate what a damaged header claims.
pub fn out_len(f: &[u8]) -> Result<(ImageInfo, usize), Error> {
    let info = header(f)?.info;
    match info.checked_buffer_len() {
        Some(n) if n <= 1 << 24 => Ok((info, n)),
        _ => Err(Error::TooLarge),
    }
}

pub fn via_threads_n(f: &[u8], threads: usize) -> Result<Vec<u8>, Error> {
    let (info, n) = out_len(f)?;
    let mut out = vec![0u8; n];
    decode_into_threads(f, &mut out, info.row_bytes(), threads)?;
    Ok(out)
}

pub fn via_threads_limits(f: &[u8], threads: usize, limits: &Limits) -> Result<Vec<u8>, Error> {
    let (info, n) = out_len(f)?;
    let mut out = vec![0u8; n];
    decode_into_threads_with_limits(f, &mut out, info.row_bytes(), threads, limits)?;
    Ok(out)
}

pub fn stream_rows(mut s: StreamDecoder<'_>, rows: &mut Vec<u8>) -> Result<(), Error> {
    let mut row = vec![0u8; s.info().row_bytes()];
    while s.next_row(&mut row)? {
        rows.extend_from_slice(&row);
    }
    Ok(())
}

pub fn via_stream(f: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    stream_rows(StreamDecoder::new(f)?, &mut out)?;
    Ok(out)
}

pub fn via_stream_limits(f: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    stream_rows(StreamDecoder::with_limits(f, limits)?, &mut out)?;
    Ok(out)
}

pub fn session(limits: Limits) -> DecodeSession {
    DecodeSession::new(DecodePolicy::default().with_output_limits(limits))
}

pub fn via_session_rows(f: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
    let (info, _) = out_len(f)?;
    let mut row = vec![0u8; info.row_bytes()];
    let mut out = Vec::new();
    session(limits).decode_rows(f, &mut row, |i, y, r| rows_into(i, &mut out, y, r))?;
    Ok(out)
}

pub fn via_session_reader(f: &[u8], limits: Limits) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    session(limits).decode_reader(Chunked { data: f, calls: 0 }, |i, y, r| {
        rows_into(i, &mut out, y, r)
    })?;
    Ok(out)
}

pub fn via_frames_limits(f: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut d = FrameDecoder::with_limits(f, limits)?;
    let mut canvas = vec![0u8; d.info().checked_buffer_len().unwrap()];
    let rb = d.info().row_bytes();
    d.next_frame()?.unwrap();
    d.decode_into(&mut canvas, rb, 2)?;
    Ok(canvas)
}

pub type Path = (&'static str, fn(&[u8]) -> Result<Vec<u8>, Error>);

/// Every decoder that reads the whole file, with checksums verified; `probe`
/// yields no pixels.
pub const PATHS: [Path; 14] = [
    ("decode", |f| decode(f).map(|d| d.1)),
    ("decode_with_limits", |f| {
        decode_with_limits(f, &Limits::DEFAULT).map(|d| d.1)
    }),
    ("decode_into_threads(1)", |f| via_threads_n(f, 1)),
    ("decode_into_threads(3)", |f| via_threads_n(f, 3)),
    ("decode_into_threads_with_limits", |f| {
        via_threads_limits(f, 4, &Limits::DEFAULT)
    }),
    ("StreamDecoder", via_stream),
    ("StreamDecoder::with_limits", |f| {
        via_stream_limits(f, &Limits::DEFAULT)
    }),
    ("DecodeSession::decode", |f| {
        session(Limits::DEFAULT).decode(f).map(|d| d.1)
    }),
    ("DecodeSession::decode_rows", |f| {
        via_session_rows(f, Limits::DEFAULT)
    }),
    ("DecodeSession::decode_reader (chunked)", |f| {
        via_session_reader(f, Limits::DEFAULT)
    }),
    ("decode_reader", |f| via_reader(f)),
    ("decode_reader (chunked)", |f| {
        via_reader(Chunked { data: f, calls: 0 })
    }),
    ("FrameDecoder", |f| via_frames_limits(f, &Limits::DEFAULT)),
    ("probe", |f| probe(f).map(|_| Vec::new())),
];

/// The same paths with verification off.
pub const OFF_PATHS: [Path; 10] = [
    ("decode_with_limits", |f| {
        decode_with_limits(f, &OFF).map(|d| d.1)
    }),
    ("decode_into_threads_with_limits(1)", |f| {
        via_threads_limits(f, 1, &OFF)
    }),
    ("decode_into_threads_with_limits(4)", |f| {
        via_threads_limits(f, 4, &OFF)
    }),
    ("StreamDecoder::with_limits", |f| via_stream_limits(f, &OFF)),
    ("DecodeSession::decode", |f| {
        session(OFF).decode(f).map(|d| d.1)
    }),
    ("DecodeSession::decode_rows", |f| via_session_rows(f, OFF)),
    ("DecodeSession::decode_reader (chunked)", |f| {
        via_session_reader(f, OFF)
    }),
    ("decode_reader", |f| via_reader_limits(f, &OFF)),
    ("decode_reader (chunked)", |f| {
        via_reader_limits(Chunked { data: f, calls: 0 }, &OFF)
    }),
    ("FrameDecoder::with_limits", |f| via_frames_limits(f, &OFF)),
];

pub fn accepted_on(paths: &[Path], f: &[u8], pixels: &[u8], what: &str) {
    for (name, path) in paths {
        let got = path(f).unwrap_or_else(|e| panic!("{name} ({what}): {e}"));
        if *name != "probe" {
            assert!(got == pixels, "{name} ({what}): pixels differ");
        }
    }
}

pub fn accepted(f: &[u8], pixels: &[u8], what: &str) {
    accepted_on(&PATHS, f, pixels, what);
}

/// Asserts every path fails on `f` (with `expected`, when given).
pub fn rejected(f: &[u8], expected: Option<&Error>, what: &str) {
    for (name, path) in PATHS {
        match path(f) {
            Ok(_) => panic!("{name} accepted {what}"),
            Err(e) => {
                if let Some(x) = expected {
                    assert_eq!(&e, x, "{name}: {what}");
                }
            }
        }
    }
}

/// (name, file without checksum, file with checksum).
pub fn files() -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
    vec![
        ("lossless", lossless(false), lossless(true)),
        ("lossy", lossy(false), lossy(true)),
    ]
}

/// The checksummed file at both placements, and with 1 KiB pieces of both
/// widths at three places (several pieces even in these small files).
pub fn variants(f: &[u8]) -> Vec<(String, Vec<u8>)> {
    let base = strip(f);
    let mut v = vec![
        ("as written".into(), f.to_vec()),
        (
            "last".into(),
            with_table(&base, f[record_at(f)], 8, Place::Last),
        ),
    ];
    for width in [4, 8] {
        for place in [Place::BeforeData, Place::Last, Place::Chunk(0)] {
            v.push((
                format!("k=10 width {width} {place:?}"),
                with_table(&base, 10, width, place),
            ));
        }
    }
    v
}
