//! `DecodeSession::decode_reader`: streamed decoding under a cumulative
//! admission policy. Rows and metadata must equal the unrestricted
//! `decode_reader`; every budget must fail closed, before rows for the
//! up-front charges, and charge consumed input even on failure.
#![cfg(feature = "std")]
use std::cell::Cell;
use std::io::Cursor;

use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::{Channels, Error, ImageInfo, Limits, Metadata, SampleType, decode_reader};

/// Deterministic textured pixels (not a smooth gradient): a hash per sample.
fn pixels(info: &ImageInfo) -> Vec<u8> {
    let n = info.checked_buffer_len().unwrap();
    let mut v = Vec::with_capacity(n);
    let mut x = 0x9e37_79b9u32;
    for i in 0..n {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        // Mostly smooth with noise, so both codings have real work to do.
        v.push(((i / 7) as u32 % 200 + (x & 31)) as u8);
    }
    v
}

fn lossless(w: u32, h: u32, ch: Channels, s: SampleType, meta: &Metadata, rows: u32) -> Vec<u8> {
    let info = ImageInfo::new(w, h, ch, s);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(rows);
        });
        cfg
    };
    cfg.encode_samples(
        imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
        meta,
    )
    .unwrap()
}

fn lossy(w: u32, h: u32, ch: Channels, groups: Option<u32>) -> Vec<u8> {
    let info = ImageInfo::new(w, h, ch, SampleType::U8);
    let mut cfg = imij::ImiEncoderConfig::lossy(90.0);
    cfg = cfg.with_quality(60.0);
    cfg = cfg.with_column_groups(groups);
    cfg.encode_samples(
        imij::ImageRef::new(&pixels(&info), info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap()
}

type Rows = Vec<(u32, Vec<u8>)>;

fn plain(file: &[u8]) -> (ImageInfo, Metadata, Rows) {
    let mut rows = Vec::new();
    let (info, meta) = decode_reader(Cursor::new(file), &Limits::DEFAULT, |_, y, r| {
        rows.push((y, r.to_vec()));
        Ok(())
    })
    .unwrap();
    (info, meta, rows)
}

fn session_rows(
    session: &mut DecodeSession<impl Fn() -> bool + Sync>,
    reader: impl std::io::Read,
) -> (Result<(ImageInfo, Metadata), Error>, Rows) {
    let mut rows = Vec::new();
    let r = session.decode_reader(reader, |_, y, r| {
        rows.push((y, r.to_vec()));
        Ok(())
    });
    (r, rows)
}

/// One byte per read, so every internal read sees a short buffer.
struct Trickle<'a>(&'a [u8]);
impl std::io::Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.0.is_empty() {
            return Ok(0);
        }
        buf[0] = self.0[0];
        self.0 = &self.0[1..];
        Ok(1)
    }
}

fn fixtures() -> Vec<(&'static str, Vec<u8>)> {
    let meta = Metadata {
        exif: Some(vec![7u8; 3000]),
        ..Metadata::default()
    };
    vec![
        (
            "lossless rgb8",
            lossless(
                97,
                61,
                Channels::Rgb,
                SampleType::U8,
                &Metadata::default(),
                8,
            ),
        ),
        (
            "lossless rgba16 + exif",
            lossless(64, 40, Channels::Rgba, SampleType::U16, &meta, 16),
        ),
        (
            "lossless gray8",
            lossless(
                33,
                70,
                Channels::Gray,
                SampleType::U8,
                &Metadata::default(),
                4,
            ),
        ),
        ("lossy rgb8", lossy(120, 72, Channels::Rgb, None)),
        ("lossy rgba8", lossy(64, 48, Channels::Rgba, None)),
        (
            "lossy rgb8 column groups",
            lossy(200, 40, Channels::Rgb, Some(64)),
        ),
    ]
}

#[test]
fn session_reader_matches_plain_reader() {
    for (name, file) in fixtures() {
        let (info, meta, want) = plain(&file);
        let mut session = DecodeSession::new(DecodePolicy::default());
        let (got, rows) = session_rows(&mut session, Cursor::new(&file));
        let (ginfo, gmeta) = got.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(ginfo, info, "{name}");
        assert_eq!(gmeta.exif, meta.exif, "{name}");
        assert_eq!(rows, want, "{name}");
        let u = session.usage();
        assert_eq!(u.input_bytes, file.len() as u64, "{name}");
        assert!(
            u.units > 0 && u.chunks > 0 && u.peak_planned_working_bytes > 0,
            "{name}"
        );
        // Short reads change nothing.
        let mut session = DecodeSession::new(DecodePolicy::default());
        let (got, rows) = session_rows(&mut session, Trickle(&file));
        assert_eq!(got.unwrap().0, info, "{name} trickle");
        assert_eq!(rows, want, "{name} trickle");
    }
}

#[test]
fn input_budget_is_cumulative_and_charges_failed_calls() {
    let file = lossless(
        97,
        61,
        Channels::Rgb,
        SampleType::U8,
        &Metadata::default(),
        8,
    );
    let len = file.len() as u64;
    let policy = DecodePolicy::default().with_max_input_bytes(len + len / 2);
    let mut session = DecodeSession::new(policy);
    assert!(session_rows(&mut session, Cursor::new(&file)).0.is_ok());
    assert_eq!(session.usage().input_bytes, len);
    // The second file does not fit in what is left: it fails having
    // consumed no more than the remaining allowance, and that is charged.
    let (r, _) = session_rows(&mut session, Cursor::new(&file));
    assert_eq!(r.unwrap_err(), Error::TooLarge);
    let used = session.usage().input_bytes;
    assert!(used > len && used <= len + len / 2, "charged {used}");
}

#[test]
fn input_budget_stops_reading_at_the_cap() {
    let file = lossy(120, 72, Channels::Rgb, None);
    let cap = file.len() as u64 / 3;
    let mut session = DecodeSession::new(DecodePolicy::default().with_max_input_bytes(cap));
    let mut rd = Cursor::new(&file);
    let (r, _) = session_rows(&mut session, &mut rd);
    assert_eq!(r.unwrap_err(), Error::TooLarge);
    assert!(
        rd.position() <= cap,
        "read {} past cap {cap}",
        rd.position()
    );
    assert!(session.usage().input_bytes <= cap);
}

#[test]
fn metadata_budget_rejects_large_retained_chunks() {
    let meta = Metadata {
        exif: Some(vec![7u8; 3000]),
        ..Metadata::default()
    };
    let file = lossless(64, 40, Channels::Rgba, SampleType::U16, &meta, 16);
    let mut tight = DecodeSession::new(DecodePolicy::default().with_max_metadata_bytes(1000));
    assert_eq!(
        session_rows(&mut tight, Cursor::new(&file)).0.unwrap_err(),
        Error::TooLarge
    );
    let mut ample = DecodeSession::new(DecodePolicy::default());
    assert!(session_rows(&mut ample, Cursor::new(&file)).0.is_ok());
    assert!(ample.usage().metadata_bytes >= 3000);
}

#[test]
fn chunk_budget_counts_every_chunk() {
    let meta = Metadata {
        exif: Some(vec![1u8; 10]),
        ..Metadata::default()
    };
    let front = lossless(16, 16, Channels::Rgb, SampleType::U8, &meta, 8);
    let info = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U8);
    let source = pixels(&info);
    let cfg =
        imij::ImiEncoderConfig::lossless().with_row_groups(imij::RowGroups::new(16, 8, 8 << 20));
    let tail = imij::v4::encode_numeric(
        info.clone(),
        &source,
        info.row_bytes(),
        &meta,
        &cfg,
        imij::v4::SpatialConfig {
            tile_columns: 1,
            tile_rows: 1,
            order: imij::v4::TileOrder::Raster,
            integrity: imij::v4::ObjectIntegrity::Objects,
            limits: imij::v4::TileLimits {
                max_object_bytes: 1 << 20,
                max_tile_bytes: 16 << 20,
            },
        },
    )
    .unwrap();
    // Front placement has eXIf + DATA. Explicit tail placement also has uIDX.
    // Every smaller cap must reject, and exact admission must count every chunk.
    for (file, count) in [(front, 2), (tail, 3)] {
        for cap in 0..count {
            let mut session = DecodeSession::new(DecodePolicy::default().with_max_chunks(cap));
            assert_eq!(
                session_rows(&mut session, Cursor::new(&file))
                    .0
                    .unwrap_err(),
                Error::TooLarge
            );
        }
        let mut exact = DecodeSession::new(DecodePolicy::default().with_max_chunks(count));
        assert!(session_rows(&mut exact, Cursor::new(&file)).0.is_ok());
        assert_eq!(exact.usage().chunks, count);
    }
}

#[test]
fn working_and_unit_budgets_fail_before_any_row() {
    for (name, file) in fixtures() {
        for policy in [
            DecodePolicy::default().with_max_working_bytes(4096),
            DecodePolicy::default().with_max_units(1),
        ] {
            let mut session = DecodeSession::new(policy);
            let (r, rows) = session_rows(&mut session, Cursor::new(&file));
            assert_eq!(r.unwrap_err(), Error::TooLarge, "{name}");
            assert!(
                rows.is_empty(),
                "{name}: {} rows before admission failed",
                rows.len()
            );
        }
    }
}

#[test]
fn output_limits_come_from_the_policy() {
    let file = lossy(120, 72, Channels::Rgb, None);
    let limits = {
        let mut limits = Limits::DEFAULT;
        limits.max_pixels = 120 * 72 - 1;
        limits
    };
    let mut session = DecodeSession::new(DecodePolicy::default().with_output_limits(limits));
    let (r, rows) = session_rows(&mut session, Cursor::new(&file));
    assert_eq!(r.unwrap_err(), Error::TooLarge);
    assert!(rows.is_empty());
}

#[test]
fn cancellation_stops_at_the_next_stripe() {
    let file = lossless(
        97,
        61,
        Channels::Rgb,
        SampleType::U8,
        &Metadata::default(),
        8,
    );
    let stop = Cell::new(false);
    // The flag is plain shared state here; a service would use an atomic.
    let flag = std::sync::atomic::AtomicBool::new(false);
    let mut session = DecodeSession::with_cancel(DecodePolicy::default(), || {
        flag.load(std::sync::atomic::Ordering::Relaxed)
    });
    let mut rows = 0u32;
    let r = session.decode_reader(Cursor::new(&file), |_, _, _| {
        rows += 1;
        if rows == 3 {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            stop.set(true);
        }
        Ok(())
    });
    assert!(stop.get());
    assert_eq!(r.unwrap_err(), Error::Cancelled);
    assert!((3..61).contains(&rows), "{rows} rows");
}
