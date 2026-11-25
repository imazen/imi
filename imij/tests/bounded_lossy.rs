//! `stream_encode::encode_lossy_bounded`: a seekable strided raster read
//! twice, decoded to `encode_lossy`'s pixels, the head unit table filled in
//! place, progress per source row and cancellation at band boundaries.

use how_far::{Outcome, StopReason};
use how_far_along::FnPulse;
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{
    BoundedLossyConfig, LossyUnitIndex, StreamEncodeError, encode_lossy_bounded,
};
use imij::{Channels, DecodeSpeed, Error, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn raster(info: &ImageInfo, stride: usize) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let mut out = vec![0xEEu8; stride * h];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let v = ((x * 5 + y * 3 + c * 70) ^ (x * y / 7)) % 256;
                out[y * stride + x * n + c] = if (x / 64 + y / 48) % 5 == 0 {
                    255
                } else {
                    v as u8
                };
            }
        }
    }
    out
}

fn packed(info: &ImageInfo, strided: &[u8], stride: usize) -> Vec<u8> {
    let rb = info.row_bytes();
    strided
        .chunks(stride)
        .flat_map(|row| row[..rb].to_vec())
        .collect()
}

#[test]
fn bounded_lossy_matches_encode_lossy_pixels_through_the_public_api() {
    let info = ImageInfo::new(1000, 333, Channels::Rgb, SampleType::U8);
    let stride = info.row_bytes() + 17;
    let src = raster(&info, stride);
    let mut lossy = imij::ImiEncoderConfig::lossy(70.0)
        .with_effort(7.5)
        .with_decode_speed(DecodeSpeed::Fast);
    lossy = lossy.with_column_groups(Some(256));
    let pulse = FnPulse::new("bounded", |_| Ok(()));
    let observer = pulse.observer();
    // The source starts at a nonzero position; the encoder rewinds to it.
    let mut prefixed = vec![7u8; 11];
    prefixed.extend_from_slice(&src);
    let mut source = Cursor::new(prefixed);
    source.set_position(11);
    let (out, scratch, sums) = encode_lossy_bounded(
        &mut source,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        None,
        info.clone(),
        &Metadata::default(),
        &lossy,
        BoundedLossyConfig::new(stride as u64, LossyUnitIndex::Reserved(4), 1 << 20),
        &pulse,
    )
    .unwrap();
    pulse.finish(Outcome::Succeeded).unwrap();
    assert_eq!(
        observer.snapshot().completed,
        2 * 333,
        "one advance per row, both passes"
    );
    assert!(!scratch.get_ref().is_empty());
    assert!(sums.is_none());
    let file = out.into_inner();
    let px = packed(&info, &src, stride);
    // `stream_encode::encode_lossy_bounded` emits V3, so compare 0-bit exact
    // against `encode_legacy_samples`.
    let whole = lossy
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let want = imij::decode(&whole).unwrap().1;
    assert_eq!(imij::decode(&file).unwrap().1, want);
    // The unit table is read with the head; a region is one batch.
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(rd.has_unit_table());
    rd.reset_read_stats();
    let r = Rect::new(500, 100, 200, 90);
    let mut got = vec![0u8; 200 * 90 * 3];
    rd.decode_region(r, &mut got, 600).unwrap();
    assert_eq!(rd.read_stats().batches, 1);
    for y in 0..90 {
        let at = ((100 + y) * 1000 + 500) * 3;
        assert_eq!(&got[y * 600..(y + 1) * 600], &want[at..at + 600]);
    }
}

#[test]
fn bounded_lossy_cancels_and_refuses_explicitly() {
    let info = ImageInfo::new(640, 400, Channels::Gray, SampleType::U8);
    let src = raster(&info, info.row_bytes());
    let lossy = imij::ImiEncoderConfig::lossy(80.0).with_effort(5.0);
    let run = |lossy: &ImiEncoderConfig, pulse: &FnPulse| {
        encode_lossy_bounded(
            &mut Cursor::new(src.clone()),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            lossy.checksum().then(|| Cursor::new(Vec::new())),
            info.clone(),
            &Metadata::default(),
            lossy,
            BoundedLossyConfig::new(info.row_bytes() as u64, LossyUnitIndex::Trailing, 1 << 20),
            pulse,
        )
        .map(|_| ())
    };
    // A stop requested after the third band check cancels at a band boundary.
    let checks = Arc::new(AtomicUsize::new(0));
    let seen = checks.clone();
    let pulse = FnPulse::new("cancel", move |_| {
        if seen.fetch_add(1, Ordering::Relaxed) >= 3 {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    });
    assert!(matches!(
        run(&lossy, &pulse),
        Err(StreamEncodeError::Codec(Error::Cancelled))
    ));
    assert!(checks.load(Ordering::Relaxed) >= 4);
    // Effort 10 runs the AC trellis with extra source passes (small images
    // there are refused: unit tests).
    let ok = FnPulse::new("refuse", |_| Ok(()));
    run(&lossy.clone().with_effort(10.0), &ok).unwrap();
    // Checksum scratch exactly when the checksum is on.
    for (checksum, scratch) in [(true, false), (false, true)] {
        assert!(matches!(
            encode_lossy_bounded(
                &mut Cursor::new(src.clone()),
                Cursor::new(Vec::new()),
                Cursor::new(Vec::new()),
                scratch.then(|| Cursor::new(Vec::new())),
                info.clone(),
                &Metadata::default(),
                &lossy.clone().with_checksum(checksum),
                BoundedLossyConfig::new(info.row_bytes() as u64, LossyUnitIndex::Trailing, 1 << 20),
                &ok,
            ),
            Err(StreamEncodeError::Codec(Error::InvalidInput(_)))
        ));
    }
    // A stride shorter than a row.
    assert!(matches!(
        encode_lossy_bounded(
            &mut Cursor::new(src.clone()),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            None,
            info.clone(),
            &Metadata::default(),
            &lossy,
            BoundedLossyConfig::new(10, LossyUnitIndex::Trailing, 1 << 20),
            &ok,
        ),
        Err(StreamEncodeError::Codec(Error::InvalidInput(_)))
    ));
}

/// A file checksum over several 1 MiB pieces: the first pieces (the head and
/// its reservation, filled last) and the streamed ones verify, and a flipped
/// byte in any piece fails the decode.
#[test]
fn bounded_lossy_streams_a_verified_file_checksum() {
    let info = ImageInfo::new(1500, 1100, Channels::Rgb, SampleType::U8);
    let mut s = 0x1234_5678u32;
    let src: Vec<u8> = (0..info.row_bytes() * 1100)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as u8
        })
        .collect();
    let lossy = imij::ImiEncoderConfig::lossy(95.0)
        .with_effort(5.0)
        .with_decode_speed(DecodeSpeed::Fast)
        .with_checksum(true);
    let want = imij::decode(
        &lossy
            .encode_legacy_samples(
                imij::ImageRef::new(&src, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap(),
    )
    .unwrap()
    .1;
    let ok = FnPulse::new("checksum", |_| Ok(()));
    for index in [LossyUnitIndex::Reserved(4), LossyUnitIndex::Trailing] {
        let (out, _, sums) = encode_lossy_bounded(
            &mut Cursor::new(src.clone()),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            Some(Cursor::new(Vec::new())),
            info.clone(),
            &Metadata::default(),
            &lossy,
            BoundedLossyConfig::new(info.row_bytes() as u64, index, 1 << 20),
            &ok,
        )
        .unwrap();
        let file = out.into_inner();
        // Covered bytes: the file without the `cSUM` chunk (tag, length
        // varint of payload + 1, payload).
        let at = file.windows(4).rposition(|w| w == b"cSUM").unwrap();
        let (mut v, mut n) = (0usize, 0);
        while file[at + 4 + n] & 0x80 != 0 {
            v |= ((file[at + 4 + n] & 0x7f) as usize) << (7 * n);
            n += 1;
        }
        v |= (file[at + 4 + n] as usize) << (7 * n);
        let covered = file.len() - (4 + n + 1 + v - 1);
        let pieces = covered.div_ceil(1 << 20);
        assert!(pieces >= 3, "{index:?}: {} bytes", file.len());
        assert_eq!(
            sums.unwrap().into_inner().len(),
            16 * (pieces - 1),
            "{index:?}"
        );
        assert!(
            RegionReader::open(SliceReader::new(&file))
                .unwrap()
                .has_file_checksum()
        );
        assert_eq!(imij::decode(&file).unwrap().1, want, "{index:?}");
        for at in [40, file.len() / 2, file.len() - 200] {
            let mut bad = file.clone();
            bad[at] ^= 0x04;
            assert!(imij::decode(&bad).is_err(), "{index:?}: flipped byte {at}");
        }
    }
}

/// A cursor whose writes fail once `limit` bytes have been written in total.
struct Failing {
    inner: Cursor<Vec<u8>>,
    limit: usize,
    written: usize,
}
impl std::io::Read for Failing {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}
impl std::io::Write for Failing {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written + buf.len() > self.limit {
            return Err(std::io::Error::other("injected write failure"));
        }
        self.written += buf.len();
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl std::io::Seek for Failing {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}
fn failing(limit: usize) -> Failing {
    Failing {
        inner: Cursor::new(Vec::new()),
        limit,
        written: 0,
    }
}

/// Output and index-scratch (exact stripes spooled ahead of the index) write
/// failures at every stage end the encode with an error, never a panic or a silently short file; a source read failure is
/// `Read`.
#[test]
fn bounded_lossy_reports_injected_io_failures() {
    let info = ImageInfo::new(420, 330, Channels::Rgb, SampleType::U8);
    // Flat panels and text-like strokes, so exact blocks qualify.
    let mut src = vec![0u8; info.row_bytes() * 330];
    for y in 0..330 {
        for x in 0..420 {
            for c in 0..3 {
                src[(y * 420 + x) * 3 + c] = match (x / 23 + y / 17) % 3 {
                    0 => 240,
                    1 if (x + y) % 3 == 0 => 20,
                    1 => 235,
                    _ => ((x * 3 + y * 2 + c * 50) % 256) as u8,
                };
            }
        }
    }
    let mut lossy = imij::ImiEncoderConfig::lossy(80.0)
        .with_effort(7.5)
        .with_decode_speed(DecodeSpeed::Fast)
        .with_checksum(true);
    lossy = lossy.tune_lossy(|t| {
        t.exact_blocks = Some(1.5);
    });
    let ok = FnPulse::new("io", |_| Ok(()));
    let cfg = BoundedLossyConfig::new(
        info.row_bytes() as u64,
        LossyUnitIndex::Reserved(4),
        1 << 20,
    );
    let run = |out: Failing, index: Failing, sums: Failing| {
        encode_lossy_bounded(
            &mut Cursor::new(src.clone()),
            out,
            index,
            Some(sums),
            info.clone(),
            &Metadata::default(),
            &lossy,
            cfg,
            &ok,
        )
        .map(|(w, i, s)| (w.inner.into_inner(), i.written, s.unwrap().written))
    };
    let big = usize::MAX / 2;
    // The file is under one checksum piece: every digest is computed from
    // held bytes at the end, so the checksum scratch stays empty here (its
    // spooling is covered by the multi-piece test above).
    let (file, index_bytes, sum_bytes) = run(failing(big), failing(big), failing(big)).unwrap();
    assert!(index_bytes > 0);
    assert_eq!(sum_bytes, 0);
    let whole = lossy
        .encode_legacy_samples(
            imij::ImageRef::new(&src, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert_eq!(
        imij::decode(&file).unwrap().1,
        imij::decode(&whole).unwrap().1
    );
    let is_err = |r: Result<(Vec<u8>, usize, usize), StreamEncodeError>| {
        matches!(
            r,
            Err(StreamEncodeError::Write(_) | StreamEncodeError::Codec(_))
        )
    };
    // Output: the header, mid-records, the last bytes.
    for at in [10, file.len() / 2, file.len() - 1] {
        assert!(
            is_err(run(failing(at), failing(big), failing(big))),
            "output at {at}"
        );
    }
    // Index scratch: inside the exact stripes, inside the index entries.
    for at in [0, index_bytes / 2, index_bytes - 1] {
        assert!(
            is_err(run(failing(big), failing(at), failing(big))),
            "index at {at}"
        );
    }
    // A source that ends early.
    let short = src[..src.len() / 2].to_vec();
    let r = encode_lossy_bounded(
        &mut Cursor::new(short),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        Some(Cursor::new(Vec::new())),
        info.clone(),
        &Metadata::default(),
        &lossy,
        cfg,
        &ok,
    );
    assert!(matches!(r, Err(StreamEncodeError::Read(_))));
}

/// A range source that waits `delay` per batch, counts batches, and fails
/// batch `fail` (1-based; 0 never).
struct Latent<'a> {
    bytes: &'a [u8],
    delay: std::time::Duration,
    batches: usize,
    fail: usize,
}
impl imij::planes::ReadAt for Latent<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.read_many_at(&mut [(offset, out)])
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.batches += 1;
        std::thread::sleep(self.delay);
        if self.batches == self.fail {
            return Err(Error::Read);
        }
        for (offset, out) in reqs.iter_mut() {
            let a = *offset as usize;
            let b = self.bytes.get(a..a + out.len()).ok_or(Error::Read)?;
            out.copy_from_slice(b);
        }
        Ok(())
    }
}

/// Region reads of bounded files (Compact: exact blocks and superblocks,
/// with a file checksum) through a source with per-batch latency: the head
/// index answers each region in one batch, and a failed batch at any point
/// is an error after which the same reader decodes the region correctly.
#[test]
fn bounded_files_read_regions_through_latent_ranges() {
    let info = ImageInfo::new(640, 520, Channels::Rgb, SampleType::U8);
    let mut src = vec![0u8; info.row_bytes() * 520];
    for y in 0..520 {
        for x in 0..640 {
            for c in 0..3 {
                src[(y * 640 + x) * 3 + c] = match (x / 23 + y / 17) % 4 {
                    0 => 240,
                    1 if (x + y) % 3 == 0 => 20,
                    1 => 235,
                    2 => ((x * 7 + y * 13 + c * 91) % 251) as u8,
                    _ => ((x * 3 + y * 2 + c * 50) % 256) as u8,
                };
            }
        }
    }
    let lossy = imij::ImiEncoderConfig::lossy(80.0)
        .with_effort(7.5)
        .with_decode_speed(DecodeSpeed::Compact)
        .with_checksum(true);
    let want = imij::decode(
        &lossy
            .encode_legacy_samples(
                imij::ImageRef::new(&src, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap(),
    )
    .unwrap()
    .1;
    let ok = FnPulse::new("latent", |_| Ok(()));
    let rects = [
        Rect::new(0, 0, 640, 64),
        Rect::new(300, 250, 200, 120),
        Rect::new(17, 449, 600, 71),
    ];
    for index in [LossyUnitIndex::Reserved(4), LossyUnitIndex::Trailing] {
        let (out, _, _) = encode_lossy_bounded(
            &mut Cursor::new(src.clone()),
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            Some(Cursor::new(Vec::new())),
            info.clone(),
            &Metadata::default(),
            &lossy,
            BoundedLossyConfig::new(info.row_bytes() as u64, index, 1 << 22),
            &ok,
        )
        .unwrap();
        let file = out.into_inner();
        let source = |fail| Latent {
            bytes: &file,
            delay: std::time::Duration::from_millis(2),
            batches: 0,
            fail,
        };
        let check = |r: Rect, got: &[u8]| {
            let w = r.width() as usize * 3;
            for y in 0..r.height() as usize {
                let at = ((r.y() as usize + y) * 640 + r.x() as usize) * 3;
                assert_eq!(
                    &got[y * w..(y + 1) * w],
                    &want[at..at + w],
                    "{index:?} {r:?}"
                );
            }
        };
        let mut rd = RegionReader::open(source(0)).unwrap();
        assert!(rd.has_unit_table() && rd.has_file_checksum());
        for r in rects {
            rd.reset_read_stats();
            let mut got = vec![0u8; r.width() as usize * r.height() as usize * 3];
            rd.decode_region(r, &mut got, r.width() as usize * 3)
                .unwrap();
            assert_eq!(rd.read_stats().batches, 1, "{index:?} {r:?}: batches");
            check(r, &got);
        }
        let opened = RegionReader::open(source(0)).unwrap().into_inner().batches;
        // A failure at each batch of opening plus one region.
        for fail in 1..=opened + 1 {
            let r = rects[1];
            let mut got = vec![0u8; r.width() as usize * r.height() as usize * 3];
            match RegionReader::open(source(fail)) {
                Err(e) => assert_eq!(e, Error::Read, "{index:?}: open batch {fail}"),
                Ok(mut rd) => {
                    let first = rd.decode_region(r, &mut got, r.width() as usize * 3);
                    assert_eq!(first, Err(Error::Read), "{index:?}: region batch {fail}");
                    rd.decode_region(r, &mut got, r.width() as usize * 3)
                        .unwrap();
                    check(r, &got);
                }
            }
        }
    }
}
