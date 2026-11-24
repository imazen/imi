//! Retained V2/V3 region decoding with the unit table (SPEC 2.7) and DATA locator
//! (SPEC 1.0): every region equals the same region of a full decode, on 1
//! and several threads, through slices and `Read + Seek` sources; the table
//! is shared by every decoder; the locator is used only when it matches the
//! chunks it names.

// V4 spatial/directory access has separate production tests in v4::region_tests
// and the real-browser HTTP gate. Keep these literal V3 locator/ordinal controls.
mod common;

use common::{chunks, extensions, header_end, put_varint, varint, with_extensions};
use imij::planes::{ReadAt, Rect, SeekReader, SliceReader};
use imij::region::{RegionReader, decode_region, decode_region_threads};
use imij::{
    Channels, DecodeSpeed, Error, FrameConfig, FrameDecoder, FrameEncoder, ImageInfo,
    ImiEncoderConfig, Limits, Metadata, SampleType, StreamDecoder, decode,
};

/// Deterministic test content: gradients, noise and flat patches, so files
/// mix predictors, runs and palette stripes.
fn pixels(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let nch = info.channels.count();
    let bps = info.sample.bytes();
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let mut out = Vec::with_capacity(w * h * nch * bps);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let flat = (x / 37 + y / 29) % 3 == 0;
                let v = if flat {
                    (c as u32 * 60 + 40) << 8
                } else {
                    ((x * 211 + y * 97 + c * 50) as u32 % 50_000) + (s % 2048)
                };
                match (info.sample, bps) {
                    (SampleType::F16, _) => {
                        // Finite halves between 0 and about 2.
                        let half = 0x3C00u16.wrapping_sub(0x1800) + (v % 0x1800) as u16;
                        out.extend_from_slice(&half.to_ne_bytes());
                    }
                    (SampleType::F32, _) => {
                        out.extend_from_slice(&(v as f32 / 65_536.0).to_ne_bytes())
                    }
                    (SampleType::F64, _) => {
                        out.extend_from_slice(&(v as f64 / 65_536.0).to_ne_bytes())
                    }
                    (_, 8) => out.extend_from_slice(
                        &((v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)).to_ne_bytes(),
                    ),
                    (_, 1) => out.push((v >> 8) as u8),
                    (_, 2) => out.extend_from_slice(&(v as u16).to_ne_bytes()),
                    _ => out.extend_from_slice(&(v.wrapping_mul(40_503)).to_ne_bytes()),
                }
            }
        }
    }
    out
}

/// Rectangles that start and end inside stripes, cross column groups,
/// touch every edge, and cover the whole image.
fn rects(w: u32, h: u32) -> Vec<Rect> {
    let mut v = vec![
        Rect::new(0, 0, w, h),
        Rect::new(0, 0, 1, 1),
        Rect::new(w - 1, h - 1, 1, 1),
        Rect::new(w / 3, h / 3, w / 3, h / 3),
        Rect::new(1, h / 2, w - 2, 1),
        Rect::new(w / 2, 0, 1, h),
        Rect::new(w - 7, h - 9, 7, 9),
    ];
    for (x, y, rw, rh) in [(5, 7, 70, 50), (60, 30, 90, 70), (120, 61, 17, 3)] {
        if x + rw <= w && y + rh <= h {
            v.push(Rect::new(x, y, rw, rh));
        }
    }
    v
}

fn crop(full: &[u8], info: &ImageInfo, r: Rect) -> Vec<u8> {
    let bpp = info.row_bytes() / info.width as usize;
    let rb = r.width() as usize * bpp;
    let mut out = Vec::with_capacity(rb * r.height() as usize);
    for y in r.y()..r.y() + r.height() {
        let a = y as usize * info.row_bytes() + r.x() as usize * bpp;
        out.extend_from_slice(&full[a..a + rb]);
    }
    out
}

/// The DATA locator record: (prefix, chunk lengths).
fn locator(file: &[u8]) -> Option<(u64, Vec<u64>)> {
    let (_, rec) = extensions(file).into_iter().find(|e| e.0 == 4)?;
    let (prefix, a) = varint(&rec);
    let (n, b) = varint(&rec[a..]);
    let mut at = a + b;
    let mut lens = Vec::new();
    for _ in 0..n {
        let (l, k) = varint(&rec[at..]);
        lens.push(l);
        at += k;
    }
    Some((prefix, lens))
}

fn locator_record(prefix: u64, lens: &[u64]) -> Vec<u8> {
    let mut b = Vec::new();
    put_varint(&mut b, prefix);
    put_varint(&mut b, lens.len() as u64);
    for &l in lens {
        put_varint(&mut b, l);
    }
    b
}

/// Checks every rectangle against the full decode through every entry
/// point and source.
fn check_regions(file: &[u8], table: bool) {
    let (info, full) = decode(file).unwrap();
    let bpp = info.row_bytes() / info.width as usize;
    let rd = RegionReader::open(SliceReader::new(file)).unwrap();
    assert_eq!(rd.has_unit_table(), table);
    assert_eq!(rd.used_locator(), table && locator(file).is_some());
    for r in rects(info.width, info.height) {
        let want = crop(&full, &info, r);
        let rb = r.width() as usize * bpp;
        for threads in [1, 3, 8] {
            let mut got = vec![0xA5u8; want.len()];
            decode_region_threads(file, r, &mut got, rb, threads).unwrap();
            assert_eq!(got, want, "{r:?}, {threads} threads");
        }
        // Strided output, the single-thread entry point and a seek source.
        let stride = rb + 5;
        let mut got = vec![0u8; stride * r.height() as usize];
        decode_region(file, r, &mut got, stride).unwrap();
        for y in 0..r.height() as usize {
            assert_eq!(
                &got[y * stride..y * stride + rb],
                &want[y * rb..(y + 1) * rb]
            );
        }
        let mut sk = RegionReader::open(SeekReader::new(std::io::Cursor::new(file))).unwrap();
        let mut got = vec![0u8; want.len()];
        sk.decode_region(r, &mut got, rb).unwrap();
        assert_eq!(got, want);
        check_planned(file, r, &want, rb);
    }
}

/// A source that, once `gate` holds ranges, serves only reads inside them:
/// what a caller holding just the planned bytes can answer.
struct Gated<'a> {
    file: &'a [u8],
    gate: std::rc::Rc<std::cell::RefCell<Option<Vec<std::ops::Range<u64>>>>>,
    batches: std::rc::Rc<std::cell::Cell<u32>>,
}
impl ReadAt for Gated<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.file.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let end = offset + out.len() as u64;
        if let Some(allowed) = &*self.gate.borrow()
            && !allowed.iter().any(|r| r.start <= offset && end <= r.end)
        {
            return Err(Error::Read);
        }
        SliceReader::new(self.file).read_exact_at(offset, out)
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.batches.set(self.batches.get() + 1);
        for (offset, out) in reqs.iter_mut() {
            self.read_exact_at(*offset, out)?;
        }
        Ok(())
    }
}

/// `region_ranges` names every byte the region's decode reads: a source
/// serving only those decodes it, twice (the second plan, after the
/// checksum pieces are verified, may be smaller). On an indexed file the
/// plan reads nothing and the decode takes one batch.
fn check_planned(file: &[u8], r: Rect, want: &[u8], rb: usize) {
    let gate = std::rc::Rc::new(std::cell::RefCell::new(None));
    let batches = std::rc::Rc::new(std::cell::Cell::new(0));
    let src = Gated {
        file,
        gate: gate.clone(),
        batches: batches.clone(),
    };
    let mut rd = RegionReader::open(src).unwrap();
    for round in 0..2 {
        let before = rd.read_stats().batches;
        let plan = rd.region_ranges(r).unwrap();
        if rd.has_unit_table() {
            assert_eq!(rd.read_stats().batches, before, "{r:?}: planning read");
        }
        assert!(plan.windows(2).all(|w| w[0].end < w[1].start), "{plan:?}");
        *gate.borrow_mut() = Some(plan);
        batches.set(0);
        let mut got = vec![0u8; want.len()];
        rd.decode_region(r, &mut got, rb).unwrap();
        assert_eq!(got, want, "{r:?}: planned decode, round {round}");
        // Unindexed files re-read a DC-only unit's record head (inside the
        // plan) before the batch.
        if rd.has_unit_table() {
            assert_eq!(batches.get(), 1, "{r:?}: round {round}");
        }
        *gate.borrow_mut() = None;
    }
}

/// Encodes `f()` with the length-prefixed layout (what main and streaming
/// writers produce).
#[cfg(feature = "lab")]
fn length_prefixed<T>(f: impl FnOnce() -> T) -> T {
    imij::lab::set_length_prefixed_layout(true);
    let r = f();
    imij::lab::set_length_prefixed_layout(false);
    r
}

fn check_disabled_extended_regions(file: &[u8], info: &ImageInfo) {
    let want = if info.sample.bytes() == 8 {
        "64-bit samples (extended tier)"
    } else if info.sample.bytes() == 4 {
        "32-bit samples (extended tier)"
    } else {
        "signed samples (extended tier)"
    };
    #[track_caller]
    fn unsupported<T>(result: Result<T, Error>, want: &'static str) {
        assert_eq!(result.err(), Some(Error::Unsupported(want)));
    }
    unsupported(decode(file), want);
    let mut full = vec![0xA5; info.row_bytes() * info.height as usize];
    for threads in [1, 3, 8] {
        unsupported(
            imij::decode_into_threads(file, &mut full, info.row_bytes(), threads),
            want,
        );
    }
    unsupported(RegionReader::open(SliceReader::new(file)), want);
    unsupported(
        RegionReader::open(SeekReader::new(std::io::Cursor::new(file))),
        want,
    );
    for r in rects(info.width, info.height) {
        let rb = r.width() as usize * info.channels.count() * info.sample.bytes();
        for stride in [rb, rb + 5] {
            let mut got = vec![0xA5; stride * r.height() as usize];
            unsupported(decode_region(file, r, &mut got, stride), want);
            for threads in [1, 3, 8] {
                unsupported(
                    decode_region_threads(file, r, &mut got, stride, threads),
                    want,
                );
            }
        }
    }
}

fn lossless_cfg(stripe_rows: u32, groups: Option<u32>) -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.tune_lossless(|t| {
        t.stripe_rows = Some(stripe_rows);
    });
    cfg = cfg.with_column_groups(groups);
    cfg
}

#[test]
fn lossless_regions_match_full_decode() {
    let cases = [
        (Channels::Rgb, SampleType::U8, 300, 260, 16, Some(64)),
        (Channels::Rgb, SampleType::U8, 300, 260, 32, None),
        (Channels::Rgba, SampleType::U8, 211, 157, 16, Some(48)),
        (Channels::Gray, SampleType::U16, 333, 190, 24, Some(100)),
        (Channels::GrayAlpha, SampleType::U16, 129, 77, 16, None),
        (Channels::Rgb, SampleType::F16, 140, 99, 16, Some(64)),
        (Channels::Rgb, SampleType::U32, 150, 80, 16, Some(64)),
        (Channels::Gray, SampleType::F32, 150, 80, 16, None),
        (Channels::Rgb, SampleType::I16, 150, 90, 16, Some(64)),
        (Channels::Gray, SampleType::I8, 120, 70, 16, None),
        (Channels::Rgba, SampleType::U64, 90, 70, 16, Some(48)),
        (Channels::Rgb, SampleType::F64, 80, 60, 16, None),
    ];
    for (i, &(ch, st, w, h, rows, groups)) in cases.iter().enumerate() {
        let info = ImageInfo::new(w, h, ch, st);
        let px = pixels(&info, i as u32 + 1);
        let cfg = lossless_cfg(rows, groups);
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        if !cfg!(feature = "extended") && st.tier() == imij::Tier::Extended {
            check_disabled_extended_regions(&file, &info);
            continue;
        }
        assert_eq!(decode(&file).unwrap().1, px, "case {i}: lossless");
        check_regions(&file, true);
        #[cfg(feature = "lab")]
        {
            let plain = length_prefixed(|| {
                cfg.encode_legacy_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap()
            });
            assert!(locator(&plain).is_none());
            assert_eq!(decode(&plain).unwrap().1, px, "case {i}: length-prefixed");
            check_regions(&plain, false);
        }
    }
}

#[test]
fn lossy_regions_match_full_decode() {
    let cases = [
        // (channels, sample, w, h, block rows per stripe, column groups, speed)
        (Channels::Rgb, SampleType::U8, 257, 211, 1, None, None),
        (Channels::Rgb, SampleType::U8, 300, 260, 2, Some(64), None),
        (
            Channels::Rgb,
            SampleType::U8,
            300,
            260,
            3,
            Some(96),
            Some(DecodeSpeed::Fast),
        ),
        (
            Channels::Rgb,
            SampleType::U8,
            200,
            180,
            1,
            Some(64),
            Some(DecodeSpeed::Compact),
        ),
        (Channels::Rgba, SampleType::U8, 211, 157, 1, Some(64), None),
        (Channels::Gray, SampleType::U8, 190, 170, 2, None, None),
        (
            Channels::GrayAlpha,
            SampleType::U16,
            140,
            99,
            1,
            Some(64),
            None,
        ),
        (
            Channels::Rgb,
            SampleType::U16,
            160,
            120,
            1,
            None,
            Some(DecodeSpeed::Fastest),
        ),
        (Channels::Rgb, SampleType::F32, 120, 100, 1, Some(64), None),
    ];
    for (i, &(ch, st, w, h, rows, groups, speed)) in cases.iter().enumerate() {
        let mut info = ImageInfo::new(w, h, ch, st);
        if st == SampleType::F32 {
            info.cicp.transfer_characteristics = 8;
        }
        let px = pixels(&info, i as u32 + 11);
        let mut cfg =
            imij::ImiEncoderConfig::lossy(80.0).tune_lossy(|t| t.stripe_block_rows = rows);
        cfg = cfg.with_column_groups(groups);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        let enc = || {
            cfg.encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap()
        };
        let file = enc();
        check_regions(&file, true);
        #[cfg(feature = "lab")]
        {
            let plain = length_prefixed(enc);
            assert_eq!(
                decode(&plain).unwrap().1,
                decode(&file).unwrap().1,
                "case {i}"
            );
            check_regions(&plain, false);
        }
    }
}

#[test]
fn single_unit_payloads_keep_the_length_prefixed_layout() {
    // One unit: no table, no locator, no feature.
    let info = ImageInfo::new(40, 30, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 3);
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(extensions(&file).is_empty());
    check_regions(&file, false);
    let lossy = imij::ImiEncoderConfig::lossy(80.0)
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(locator(&lossy).is_none());
    check_regions(&lossy, false);
}

/// The original read-threshold test on the unit table: after open, a region
/// reads exactly its unit's record, in one batch.
#[test]
fn reads_only_what_the_region_needs() {
    let info = ImageInfo::new(640, 512, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 5);
    let cfg = lossless_cfg(16, Some(128));
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let r = Rect::new(300, 400, 32, 16);
    let mut out = vec![0u8; 32 * 16 * 3];
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    rd.reset_read_stats();
    rd.decode_region(r, &mut out, 96).unwrap();
    let with = rd.read_stats();
    assert_eq!(with.batches, 1, "{with:?}");
    assert!(with.reads <= 2, "{with:?}");
    assert!(
        with.bytes * 20 < file.len() as u64,
        "{with:?} of {}",
        file.len()
    );
    // Exactly the record of the one unit it needs (its span in the hint):
    // stripe 400 / 16 = 25, column group 300 / 128 = 2 of 5.
    assert_eq!(with.bytes, unit_lengths(&file)[25 * 5 + 2]);
    #[cfg(feature = "lab")]
    {
        let plain = length_prefixed(|| {
            cfg.encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap()
        });
        let mut rd = RegionReader::open(SliceReader::new(&plain)).unwrap();
        // The record-size walk, at open (so trailing bytes are found as a
        // full decode finds them): one read per record.
        let walk = rd.read_stats();
        assert!(walk.reads > 32 * 5, "{walk:?}");
        // Regions reuse it: one read of the unit's record.
        rd.reset_read_stats();
        rd.decode_region(r, &mut out, 96).unwrap();
        assert_eq!(rd.read_stats().reads, 1);
        rd.reset_read_stats();
        rd.decode_region(Rect::new(0, 0, 8, 8), &mut out, 24)
            .unwrap();
        assert_eq!(rd.read_stats().reads, 1);
    }
}

/// The unit table hint of a file: every record's whole bytes (its span).
fn unit_lengths(file: &[u8]) -> Vec<u64> {
    let h = imij::header(file).unwrap();
    let d = &file[common::data_payload(file)];
    let t = common::payload(d, &h.info, h.coding == imij::Coding::Lossy)
        .table()
        .expect("a unit table hint");
    let mut at = t.start;
    let mut v = Vec::new();
    while at < t.end {
        let (l, n) = varint(&d[at..]);
        v.push(l);
        at += n;
    }
    v
}

/// ICC, Exif and XMP of 3, `exif_kb` and 200 KiB (deterministic noise).
fn metadata_kb(exif_kb: usize) -> Metadata {
    let mut s = 0x9E37_79B9u32;
    let mut fill = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as u8
            })
            .collect()
    };
    Metadata {
        icc: Some(fill(3 * 1024)),
        exif: Some(fill(exif_kb * 1024)),
        xmp: Some(fill(200 * 1024)),
        ..Default::default()
    }
}

fn metadata() -> Metadata {
    metadata_kb(60)
}

/// The locator names every chunk before DATA, so opening a file with large
/// metadata takes two batches; one that disagrees with the chunks is not
/// used (a sequential walk finds DATA), so it can never select other bytes.
#[test]
fn locator_is_checked_against_the_chunks() {
    let info = ImageInfo::new(640, 300, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 44);
    let cfg = lossless_cfg(16, Some(128));
    // Exif ends past the first 64 KiB: the xMPp header needs a read too.
    let meta = metadata_kb(100);
    let file = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    let full = decode(&file).unwrap().1;
    let r = Rect::new(130, 100, 200, 90);
    let want = crop(&full, &info, r);
    let (prefix, lens) = locator(&file).expect("a multi-unit file has a locator");
    assert_eq!(lens.len(), 3, "iCCP, eXIf, xMPp");
    let readable = |f: &[u8], located: bool, batches: u64, what: &str| {
        assert_eq!(decode(f).unwrap().1, full, "{what}: decode");
        let mut rd = RegionReader::open(SliceReader::new(f)).unwrap();
        assert!(rd.has_unit_table(), "{what}");
        assert_eq!(rd.used_locator(), located, "{what}: locator use");
        assert_eq!(rd.read_stats().batches, batches, "{what}: open batches");
        let mut got = vec![0u8; want.len()];
        rd.reset_read_stats();
        rd.decode_region(r, &mut got, 600).unwrap();
        assert_eq!(got, want, "{what}: region");
        assert_eq!(rd.read_stats().batches, 1, "{what}: region batches");
    };
    // The first 64 KiB, then the xMPp header and the DATA prefix together,
    // then the record heads past those bytes (the hint check, SPEC 2.7).
    readable(&file, true, 3, "as written");
    // Without the locator: the first window, the xMPp header's window, then
    // DATA's (which holds its head and table), then the record heads past it
    // (the hint check, SPEC 2.7).
    let none = with_extensions(
        &file,
        &extensions(&file)
            .into_iter()
            .filter(|e| e.0 != 4)
            .collect::<Vec<_>>(),
    );
    assert!(locator(&none).is_none());
    readable(&none, false, 4, "no locator");
    // A locator that disagrees costs its one batch, then the walk, then the
    // record heads (the hint check, SPEC 2.7).
    let ext = |prefix: u64, lens: &[u64]| {
        let mut recs: Vec<(u64, Vec<u8>)> =
            extensions(&file).into_iter().filter(|e| e.0 != 4).collect();
        recs.push((4, locator_record(prefix, lens)));
        with_extensions(&file, &recs)
    };
    // A wrong chunk length, a wrong DATA prefix length, a missing chunk.
    let mut bad = lens.clone();
    bad[2] += 7;
    readable(&ext(prefix, &bad), false, 5, "wrong chunk length");
    readable(&ext(prefix + 1, &lens), false, 5, "wrong prefix");
    readable(&ext(prefix - 1, &lens), false, 5, "short prefix");
    readable(&ext(prefix, &lens[..2]), false, 5, "missing chunk");
    // A fake DATA chunk inside the XMP payload, and a locator that points at
    // it: the xMPp header disagrees with the listed lengths, so the reader
    // walks to the real DATA.
    let black = vec![0u8; px.len()];
    let fake = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&black, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (_, fa, fb) = chunks(&fake).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let mut xmp = meta.xmp.clone().unwrap();
    xmp[1000..1000 + (fb - fa)].copy_from_slice(&fake[fa..fb]);
    let meta2 = Metadata {
        xmp: Some(xmp),
        ..meta.clone()
    };
    let lured = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta2)
        .unwrap();
    let (lp, ll) = locator(&lured).unwrap();
    let xmp_payload_at = header_end(&lured) + (ll[0] + ll[1]) as usize + 4 + 3;
    assert_eq!(&lured[xmp_payload_at + 1000..][..4], b"DATA");
    let fake_prefix = {
        let (_, fl) = locator(&fake).unwrap_or((0, Vec::new()));
        assert!(fl.is_empty());
        locator(&fake).unwrap().0
    };
    let mut recs: Vec<(u64, Vec<u8>)> = extensions(&lured)
        .into_iter()
        .filter(|e| e.0 != 4)
        .collect();
    let lure_lens = [ll[0], ll[1], 4 + 3 + 1000];
    recs.push((4, locator_record(fake_prefix, &lure_lens)));
    let lured = with_extensions(&lured, &recs);
    let mut rd = RegionReader::open(SliceReader::new(&lured)).unwrap();
    assert!(!rd.used_locator());
    let mut got = vec![0u8; want.len()];
    rd.decode_region(r, &mut got, 600).unwrap();
    assert_eq!(got, want, "the real DATA, not the lure");
    assert_eq!(lp, prefix);
    // A metadata remux (compressible XMP) changes the chunks before DATA;
    // the locator stays in step.
    let text = Metadata {
        xmp: Some(b"<rdf:li>region</rdf:li>".repeat(9000)),
        ..meta.clone()
    };
    let wordy = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &text)
        .unwrap();
    let remuxed = imij::compress_metadata(&wordy, 1 << 20).unwrap();
    assert!(remuxed.len() + 100_000 < wordy.len());
    assert_ne!(locator(&remuxed), locator(&wordy));
    let mut rd = RegionReader::open(SliceReader::new(&remuxed)).unwrap();
    assert!(rd.used_locator());
    let mut got = vec![0u8; want.len()];
    rd.decode_region(r, &mut got, 600).unwrap();
    assert_eq!(got, want, "remuxed");
}

/// The unit table hint is checked by every route (SPEC 2.7). A hint whose
/// entries only add up (the 2026-10-08 review's witness, now a hint), or
/// with two entries swapped, is corrupt for every route: the in-memory
/// decoders check it before any row, `decode_reader` as it reaches each
/// stripe (rows of earlier stripes may come first, SPEC 1.1), and the region
/// reader against every span's record head when it opens the file
/// (review 2026-10-10 #1).
#[test]
fn every_route_checks_the_unit_table_hint() {
    let info = ImageInfo::new(32, 64, Channels::Gray, SampleType::U8);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    // Four stripes of equal records, and four whose records differ.
    let even: Vec<u8> = (0..2048)
        .map(|x| ((x / 512) * 50 + (x % 32)) as u8)
        .collect();
    let uneven: Vec<u8> = (0..2048u32)
        .map(|x| {
            if (x / 512) % 2 == 0 {
                (x % 32) as u8
            } else {
                (x * 37 % 251) as u8
            }
        })
        .collect();
    let mismatch = Err(Error::Corrupt("unit table does not match the records"));
    for (px, forge) in [
        // [l, l, l, l] -> [2l, l, 2, l - 2]: unit 1's span is exactly unit
        // 2's record, and every span's total still fills DATA.
        (&even, forge_shift as fn(&[u8]) -> Vec<u8>),
        // Two entries swapped: the same total, every span a different size.
        (&uneven, forge_swap),
    ] {
        let file = cfg
            .encode_legacy_samples(imij::ImageRef::new(px, 32, &info), &Metadata::default())
            .unwrap();
        let d = common::data_payload(&file);
        let t = common::payload(&file[d.clone()], &info, false)
            .table()
            .expect("a unit table hint");
        let (t0, t1) = (d.start + t.start, d.start + t.end);
        let spans = file[t0..t1].to_vec();
        // One byte per entry (each under 128).
        assert_eq!(spans.len(), 4);
        let mut forged = file.clone();
        forged[t0..t1].copy_from_slice(&forge(&spans));
        assert_ne!(forged, file);
        assert_eq!(decode(&forged).map(|_| ()), mismatch);
        assert_eq!(StreamDecoder::new(&forged).map(|_| ()), mismatch);
        // The reader delivers rows stripe by stripe, so rows of stripes
        // before the mismatch may come first (SPEC 1.1); both forgeries
        // break at unit 0.
        let r = imij::decode_reader(&forged[..], &imij::Limits::DEFAULT, |_, _, _| Ok(()));
        assert_eq!(r.map(|_| ()), mismatch);
        // The region reader checks every span at open.
        assert_eq!(
            RegionReader::open(SliceReader::new(&forged)).map(|_| ()),
            mismatch
        );
        for s in 0..4 {
            let r = Rect::new(0, 16 * s, 32, 16);
            let mut out = vec![0u8; 512];
            assert_eq!(
                decode_region(&forged, r, &mut out, 32).map(|_| ()),
                mismatch,
                "stripe {s}"
            );
        }
    }
    // A hint whose total disagrees with DATA's length is corrupt for all.
    let file = cfg
        .encode_legacy_samples(imij::ImageRef::new(&even, 32, &info), &Metadata::default())
        .unwrap();
    let d = common::data_payload(&file);
    let t = common::payload(&file[d.clone()], &info, false)
        .table()
        .unwrap();
    let mut short = file.clone();
    short[d.start + t.end - 1] -= 1;
    assert!(matches!(decode(&short), Err(Error::Corrupt(_))));
    assert!(matches!(
        RegionReader::open(SliceReader::new(&short)),
        Err(Error::Corrupt(_))
    ));
}

/// [l, l, l, l] -> [2l, l, 2, l - 2].
fn forge_shift(s: &[u8]) -> Vec<u8> {
    assert!(s.iter().all(|&l| l == s[0] && l < 64), "{s:?}");
    vec![2 * s[0], s[0], 2, s[0] - 2]
}

/// The first two entries swapped.
fn forge_swap(s: &[u8]) -> Vec<u8> {
    assert_ne!(s[0], s[1], "{s:?}");
    vec![s[1], s[0], s[2], s[3]]
}

/// A source that counts batches the way an object store would serve them.
struct Counting<'a> {
    bytes: &'a [u8],
    batches: Vec<usize>,
}

impl ReadAt for Counting<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.batches.push(1);
        let a = offset as usize;
        out.copy_from_slice(&self.bytes[a..a + out.len()]);
        Ok(())
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.batches.push(reqs.len());
        for (offset, out) in reqs.iter_mut() {
            let a = *offset as usize;
            out.copy_from_slice(&self.bytes[a..a + out.len()]);
        }
        Ok(())
    }
}

#[test]
fn reads_arrive_in_batches() {
    let info = ImageInfo::new(1024, 512, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 12);
    let cfg = lossless_cfg(16, Some(128));
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &metadata(),
        )
        .unwrap();
    let src = Counting {
        bytes: &file,
        batches: Vec::new(),
    };
    let mut rd = RegionReader::open(src).unwrap();
    let r = Rect::new(100, 50, 600, 300);
    let mut out = vec![0u8; 600 * 300 * 3];
    rd.decode_region(r, &mut out, 1800).unwrap();
    assert_eq!(out, crop(&decode(&file).unwrap().1, &info, r));
    let stats = rd.read_stats();
    let src = rd.into_inner();
    // Open: the first window, then one batch, then the record heads past
    // those bytes (the hint check, SPEC 2.7); the region: one batch of one
    // run per stripe (the region spans 5 of 8 column groups).
    assert_eq!(src.batches.len(), 4, "{:?}", src.batches);
    assert_eq!(stats.batches, 4);
    assert_eq!(src.batches[3], 350usize.div_ceil(16) - 50 / 16);
}

/// The region reader applies the chunk rules every decoder applies to the
/// chunks before DATA (review of PR #11, finding 8: it applied only the tag
/// and criticality rules, so a repeated eXIf, a bad mDCv or cLLI, a bad
/// compressed metadata record or the mixed-pages signal without aNIM
/// decoded here while every full decoder refused the file; the matrix pins
/// those cases on every route). The rules a chunk header shows are checked
/// at open; an mCMP record's syntax with the first region, whose batch
/// brings the record, so the rules cost bytes, not batches.
#[test]
fn region_reads_follow_the_chunk_rules() {
    let info = ImageInfo::new(1024, 512, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 12);
    let cfg = lossless_cfg(16, Some(128));
    // Noise Exif keeps its bytes; the text XMP compresses into an mCMP
    // chunk past the first 64 KiB.
    let meta = Metadata {
        xmp: Some(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf/></x:xmpmeta>".repeat(4000)),
        ..metadata_kb(100)
    };
    let plain = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    let packed = imij::compress_metadata(&plain, 1 << 20).unwrap();
    let (_, at, end) = chunks(&packed)
        .into_iter()
        .find(|c| &c.0 == b"mCMP")
        .expect("compressed XMP");
    assert!(at > 65536, "mCMP at {at}");
    let r = Rect::new(100, 50, 600, 300);
    let want = crop(&decode(&packed).unwrap().1, &info, r);
    let region = |file: &[u8]| {
        let src = Counting {
            bytes: file,
            batches: Vec::new(),
        };
        RegionReader::open(src).map(|mut rd| {
            let mut out = vec![0u8; 600 * 300 * 3];
            let got = rd.decode_region(r, &mut out, 1800).map(|_| out);
            (got, rd.into_inner().batches.len())
        })
    };
    let (got, batches) = region(&packed).unwrap();
    assert_eq!(got.unwrap(), want);
    // Open: the first window, one batch and the record heads past those
    // bytes (the hint check, SPEC 2.7); the region one more.
    assert_eq!(batches, 4);
    // A bad record: every full decoder refuses it; the region reader never
    // fetches metadata payloads (SPEC 1.1: a partial decoder validates the
    // bytes it reads), so it decodes the region in the same four batches.
    let mut bad = packed.clone();
    let payload = at + 4 + varint(&packed[at + 4..]).1;
    bad[payload + 4..end].fill(0xff);
    assert_eq!(
        decode(&bad).err(),
        Some(Error::Unsupported("metadata pack method"))
    );
    let (got, batches) = region(&bad).unwrap();
    assert_eq!((got.unwrap(), batches), (want.clone(), 4));
    // A second Exif chunk fails at open, as in every decoder.
    let mut twice = plain.clone();
    let (_, x, _) = chunks(&plain)
        .into_iter()
        .find(|c| &c.0 == b"xMPp")
        .unwrap();
    twice[x..x + 4].copy_from_slice(b"eXIf");
    const REPEATED: Error = Error::Corrupt("repeated chunk");
    assert_eq!(decode(&twice).err(), Some(REPEATED));
    assert_eq!(region(&twice).err(), Some(REPEATED));
}

#[test]
fn pages_carry_their_own_tables() {
    let info = ImageInfo::new(200, 150, Channels::Rgb, SampleType::U8);
    let cfg = FrameConfig::new(lossless_cfg(16, None));
    let mut enc = FrameEncoder::pages(info.clone(), metadata(), cfg);
    let pages: Vec<Vec<u8>> = (0..3).map(|i| pixels(&info, 20 + i)).collect();
    for p in &pages {
        enc.push_page(p, info.row_bytes(), &info).unwrap();
    }
    let file = enc.finish_legacy().unwrap();
    let mut dec = FrameDecoder::new(&file).unwrap();
    let mut canvas = vec![0u8; info.row_bytes() * 150];
    let mut i = 0;
    while dec.next_frame().unwrap().is_some() {
        dec.decode_into(&mut canvas, info.row_bytes(), 1).unwrap();
        assert_eq!(canvas, pages[i]);
        i += 1;
    }
    assert_eq!(i, 3);
    // The outer file's locator names aNIM too (inserted after page 1 was
    // coded) and stays in step.
    let (_, lens) = locator(&file).unwrap();
    assert_eq!(lens.len(), 4, "iCCP, eXIf, xMPp, aNIM");
    let r = Rect::new(50, 40, 60, 70);
    let mut got = vec![0u8; 60 * 70 * 3];
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(rd.used_locator() && rd.has_unit_table());
    rd.decode_region(r, &mut got, 180).unwrap();
    assert_eq!(got, crop(&pages[0], &info, r));
}

/// The region reader decodes the primary image and stops at the end of
/// `DATA` (SPEC 1.1): it does not read the chunks after it. Full decoders
/// still refuse a second `DATA` or an unknown critical chunk there; the
/// region reader decodes the primary image of those files, from the same
/// bytes a full decode of it uses.
#[test]
fn the_region_reader_stops_at_the_end_of_data() {
    let info = ImageInfo::new(256, 128, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 31);
    let file = lossless_cfg(16, Some(64))
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let (_, a, b) = chunks(&file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let mut two = file.clone();
    two.extend_from_slice(&file[a..b]);
    assert!(decode(&two).is_err());
    let mut extra = file.clone();
    extra.extend_from_slice(b"zZZz\x04abc");
    assert_eq!(decode(&extra).unwrap().1, px);
    let mut critical = file.clone();
    critical.extend_from_slice(b"ZZZz\x04abc");
    assert!(matches!(decode(&critical), Err(Error::Unsupported(_))));
    let whole = Rect::new(0, 0, 256, 128);
    for (what, f) in [
        ("second DATA", &two),
        ("ancillary", &extra),
        ("critical", &critical),
    ] {
        let mut rd = RegionReader::open(SliceReader::new(f)).unwrap();
        // Opening reads the first window (the header and DATA's head and
        // table) and nothing after it.
        assert_eq!(rd.read_stats().batches, 1, "{what}");
        let mut got = vec![0u8; px.len()];
        rd.decode_region(whole, &mut got, info.row_bytes()).unwrap();
        assert_eq!(got, px, "{what}");
    }
}

/// Pages and animation frames follow `DATA` as `fRAM` chunks; a region of
/// the primary image does not visit them, so opening takes the same two
/// batches however many follow (review of PR #11, finding 5: 100 pages took
/// 100 batches).
#[test]
fn pages_and_animations_open_in_two_batches() {
    let info = ImageInfo::new(128, 64, Channels::Gray, SampleType::U8);
    let frames: Vec<Vec<u8>> = (0..100).map(|i| pixels(&info, 300 + i)).collect();
    let cfg = || FrameConfig::new(lossless_cfg(16, None));
    let mut pages = FrameEncoder::pages(info.clone(), metadata(), cfg());
    let mut anim = FrameEncoder::animation(info.clone(), metadata(), cfg());
    for f in &frames {
        pages.push_page(f, 128, &info).unwrap();
        anim.push_frame(f, 128, imij::FrameDuration::from_millis(40))
            .unwrap();
    }
    let r = Rect::new(10, 20, 100, 30);
    for (what, file) in [
        ("pages", pages.finish_legacy().unwrap()),
        ("animation", anim.finish_legacy().unwrap()),
    ] {
        assert!(file.len() > 4 * 65536, "{what}: {} bytes", file.len());
        let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
        assert!(rd.used_locator() && rd.has_unit_table(), "{what}");
        // Two batches, and one more for the record heads past them (the
        // hint check, SPEC 2.7).
        assert!(
            rd.read_stats().batches <= 3,
            "{what}: {:?}",
            rd.read_stats()
        );
        let mut got = vec![0u8; 100 * 30];
        rd.decode_region(r, &mut got, 100).unwrap();
        assert_eq!(got, crop(&frames[0], &info, r), "{what}");
    }
}

#[test]
fn overflowing_output_stride_is_rejected_before_writing() {
    let info = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 9);
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut reader = RegionReader::open(SliceReader::new(&file)).unwrap();
    let mut out = [0xA5; 96];
    assert_eq!(
        reader.decode_region(Rect::new(0, 0, 8, 3), &mut out, usize::MAX),
        Err(Error::InvalidInput("output buffer too small"))
    );
    assert_eq!(out, [0xA5; 96]);
    // Regions outside the image and short buffers are input errors.
    assert!(matches!(
        decode_region(&file, Rect::new(10, 0, 7, 1), &mut out, 48),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        decode_region(&file, Rect::new(0, 0, 10, 10), &mut out[..10], 30),
        Err(Error::InvalidInput(_))
    ));
}

/// Exact blocks (Compact 0.1, docs/LOSSY.md "Exact blocks"): the exact head
/// places every nested stripe (its unit's record and offset) and gives every
/// unit's first list entry, so a region fetches the stripes its units need,
/// in whichever unit's record they sit, in the same batch as its units'
/// records, and equals the full decode on every route, with and without
/// column groups, alpha and the Compact tier's default nested stripes.
#[test]
fn exact_block_regions_match_full_decode() {
    // (w, h, channels, block rows per stripe (0 = the tier's), column
    // groups, tier, exact ratio (None = the tier's default)).
    type Case = (
        u32,
        u32,
        Channels,
        u32,
        Option<u32>,
        Option<DecodeSpeed>,
        Option<f32>,
    );
    let cases: [Case; 5] = [
        (256, 192, Channels::Rgb, 1, None, None, Some(1000.0)),
        (300, 260, Channels::Rgb, 2, Some(64), None, Some(1.5)),
        (
            300,
            200,
            Channels::Rgba,
            0,
            None,
            Some(DecodeSpeed::Compact),
            None,
        ),
        (
            600,
            530,
            Channels::Rgb,
            0,
            None,
            Some(DecodeSpeed::Compact),
            Some(1000.0),
        ),
        (
            200,
            180,
            Channels::Rgb,
            1,
            Some(64),
            Some(DecodeSpeed::Compact),
            Some(3.0),
        ),
    ];
    for (i, &(w, h, ch, rows, groups, speed, ratio)) in cases.iter().enumerate() {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = pixels(&info, 61 + i as u32);
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        if rows != 0 {
            cfg = cfg.tune_lossy(|t| t.stripe_block_rows = rows);
        }
        cfg = cfg.with_column_groups(groups);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = ratio;
        });
        let enc = || {
            cfg.encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap()
        };
        let file = enc();
        let listed = common::feature_mask(&file);
        assert_ne!(
            listed & common::feature::EXACT_BLOCKS,
            0,
            "case {i}: {listed:#x}"
        );
        check_regions(&file, true);
        #[cfg(feature = "lab")]
        {
            let plain = length_prefixed(enc);
            assert_eq!(
                decode(&plain).unwrap().1,
                decode(&file).unwrap().1,
                "case {i}"
            );
            check_regions(&plain, false);
        }
    }
    // A region inside the first stripe (which starts a DC stripe) of a file
    // without column groups reads that unit's records and nothing else:
    // its DC, lossy and exact records, in one batch.
    let info = ImageInfo::new(256, 192, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 61);
    let mut cfg = imij::ImiEncoderConfig::lossy(80.0).tune_lossy(|t| t.stripe_block_rows = 1);
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = Some(1000.0);
    });
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    rd.reset_read_stats();
    let mut got = vec![0u8; 50 * 5 * 3];
    rd.decode_region(Rect::new(10, 2, 50, 5), &mut got, 150)
        .unwrap();
    let stats = rd.read_stats();
    let lens = unit_lengths(&file);
    assert_eq!(stats.batches, 1, "{stats:?}");
    assert_eq!(stats.bytes, lens[0] + lens[1] + lens[2], "{stats:?}");
    let full = decode(&file).unwrap().1;
    assert_eq!(got, crop(&full, &info, Rect::new(10, 2, 50, 5)));
    // A unit whose exact blocks start inside a nested stripe an earlier unit
    // carries (16-row nested stripes hold 64 blocks, the 8-row units 32):
    // unit 1 needs stripe 0, which unit 0's exact record carries. The
    // region still takes one batch and fetches unit 1's records plus that
    // stripe, less than units 0 and 1 together.
    rd.reset_read_stats();
    rd.decode_region(Rect::new(10, 10, 50, 5), &mut got, 150)
        .unwrap();
    let stats = rd.read_stats();
    assert_eq!(stats.batches, 1, "{stats:?}");
    assert!(stats.bytes > lens[3] + lens[4], "{stats:?}");
    assert!(
        stats.bytes < lens[0] + lens[1] + lens[2] + lens[3] + lens[4],
        "{stats:?}"
    );
    assert_eq!(got, crop(&full, &info, Rect::new(10, 10, 50, 5)));
}

/// Mixed pages (aNIM kind 3): the feature `mixed_pages` joins the first
/// page's feature mask and the locator (header record 4) its records.
#[test]
fn mixed_pages_keep_header_extensions_in_order() {
    let a = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let b = ImageInfo::new(64, 40, Channels::Gray, SampleType::U16);
    let (pa, pb) = (pixels(&a, 71), pixels(&b, 72));
    let cfg = FrameConfig::new(lossless_cfg(16, Some(64)));
    let mut enc = FrameEncoder::mixed_pages(cfg);
    enc.push_page(&pa, a.row_bytes(), &a).unwrap();
    enc.push_page(&pb, b.row_bytes(), &b).unwrap();
    let file = enc.finish_legacy().unwrap();
    let ids: Vec<u64> = extensions(&file).iter().map(|e| e.0).collect();
    assert_eq!(ids, [4], "locator");
    assert_eq!(
        common::feature_mask(&file) & common::feature::MIXED_PAGES,
        common::feature::MIXED_PAGES
    );
    assert_eq!(locator(&file).unwrap().1.len(), 1, "aNIM");
    assert_eq!(decode(&file).unwrap().1, pa);
    let mut dec = FrameDecoder::new(&file).unwrap();
    let mut seen = 0;
    while dec.next_frame().unwrap().is_some() {
        let info = dec.frame_info().unwrap();
        let mut out = vec![0u8; info.row_bytes() * info.height as usize];
        dec.decode_into(&mut out, info.row_bytes(), 1).unwrap();
        assert_eq!(out, [&pa, &pb][seen][..]);
        seen += 1;
    }
    assert_eq!(seen, 2);
    let r = Rect::new(30, 40, 100, 90);
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(rd.used_locator() && rd.has_unit_table());
    let mut got = vec![0u8; 100 * 90 * 3];
    rd.decode_region(r, &mut got, 300).unwrap();
    assert_eq!(got, crop(&pa, &a, r));
}

/// Science records (SPEC 1.5) are ancillary chunks before DATA: the
/// locator names them like any chunk, and a region read skips them.
#[test]
fn science_chunks_are_named_by_the_locator() {
    use imij::science::{ChannelCalibration, ScienceRecords};
    let info = ImageInfo::new(240, 160, Channels::Gray, SampleType::U16);
    let px = pixels(&info, 81);
    let mut c = ChannelCalibration::default();
    c.offset = -1024.0;
    c.quantity = Some("HU".into());
    let mut records = ScienceRecords::default();
    records.calibration.push(c);
    let mut meta = Metadata::default();
    records.write_metadata(&info, &mut meta).unwrap();
    let file = lossless_cfg(16, Some(64))
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    let before_data: Vec<[u8; 4]> = chunks(&file)
        .into_iter()
        .map(|c| c.0)
        .take_while(|t| t != b"DATA")
        .collect();
    assert!(before_data.contains(b"cALB"), "{before_data:?}");
    assert_eq!(locator(&file).unwrap().1.len(), before_data.len());
    let r = Rect::new(20, 30, 90, 70);
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(rd.used_locator());
    let mut got = vec![0u8; 90 * 70 * 2];
    rd.decode_region(r, &mut got, 180).unwrap();
    assert_eq!(got, crop(&px, &info, r));
    let (info2, meta2) = imij::probe(&file).unwrap();
    assert_eq!(
        ScienceRecords::from_metadata(&info2, &meta2).unwrap(),
        records
    );
}

/// Piece checksums (SPEC 1.4): the `cSUM` chunk before DATA is one more
/// chunk the locator names, and the checksum covers the locator. A
/// checksummed file verifies on every decoder and its locator holds; the
/// same after a metadata remux and for a mixed-pages document.
#[test]
fn checksums_and_the_locator_agree() {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 91);
    let rb = info.row_bytes();
    let cfg = lossless_cfg(16, Some(64)).with_checksum(true);
    let r = Rect::new(40, 30, 120, 90);
    let want = crop(&px, &info, r);
    let check = |file: &[u8], ids: &[u64], what: &str| {
        let got: Vec<u64> = extensions(file).iter().map(|e| e.0).collect();
        assert_eq!(got, ids, "{what}: extension ids");
        let before: Vec<[u8; 4]> = chunks(file)
            .into_iter()
            .map(|c| c.0)
            .take_while(|t| t != b"DATA")
            .collect();
        assert!(before.contains(b"cSUM"), "{what}: {before:?}");
        assert_eq!(locator(file).unwrap().1.len(), before.len(), "{what}");
        assert_eq!(
            decode(file).unwrap().1[..rb * 200],
            px[..],
            "{what}: decode"
        );
        let mut out = vec![0u8; px.len()];
        imij::decode_into_threads(file, &mut out, rb, 4).unwrap();
        assert_eq!(out, px, "{what}: threads");
        let mut rows = Vec::new();
        imij::decode_reader(
            std::io::Cursor::new(file),
            &imij::Limits::DEFAULT,
            |_, _, r| {
                rows.extend_from_slice(r);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rows, px, "{what}: reader");
        let mut rd = RegionReader::open(SliceReader::new(file)).unwrap();
        assert!(rd.used_locator(), "{what}: locator");
        let mut got = vec![0u8; want.len()];
        rd.decode_region(r, &mut got, 360).unwrap();
        assert_eq!(got, want, "{what}: region");
    };
    // Compressible XMP, so the remux below rewrites the chunks before DATA.
    let meta = Metadata {
        xmp: Some(b"<rdf:li>region</rdf:li>".repeat(4000)),
        ..metadata_kb(10)
    };
    let file = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta)
        .unwrap();
    check(&file, &[2, 4], "encode");
    // The locator is covered: one changed byte in it is a checksum mismatch.
    let (_, loc) = common::extension_ranges(&file)
        .into_iter()
        .find(|e| e.0 == 4)
        .unwrap();
    let mut bad = file.clone();
    bad[loc.start] ^= 1;
    assert_eq!(
        decode(&bad).err(),
        Some(Error::Corrupt("file checksum mismatch"))
    );
    let remuxed = imij::compress_metadata(&file, 1 << 20).unwrap();
    assert!(remuxed.len() < file.len());
    check(&remuxed, &[2, 4], "remux");
    let mut enc = FrameEncoder::mixed_pages(FrameConfig::new(cfg));
    let b = ImageInfo::new(64, 40, Channels::Gray, SampleType::U16);
    enc.push_page(&px, rb, &info).unwrap();
    enc.push_page(&pixels(&b, 92), b.row_bytes(), &b).unwrap();
    check(&enc.finish_legacy().unwrap(), &[2, 4], "mixed pages");
}

/// Near-lossless heads (version 2, feature `near_lossless`) carry bounds
/// after the channel maps; the unit table flag sits after them. A
/// near-lossless file with a table declares `near_lossless`, decodes on
/// every path, and its regions equal the full decode's.
#[test]
fn near_lossless_files_have_tables_and_regions() {
    for (ch, st) in [
        (Channels::Rgb, SampleType::U8),
        (Channels::Gray, SampleType::U16),
    ] {
        let info = ImageInfo::new(260, 150, ch, st);
        let px = pixels(&info, 101);
        let cfg = {
            let mut cfg = lossless_cfg(16, Some(64));
            cfg = cfg.tune_lossless(|t| {
                t.max_error = [2; 4];
            });
            cfg
        };
        let file = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(common::feature_mask(&file), common::feature::NEAR_LOSSLESS);
        let d = &file[common::data_payload(&file)];
        let payload = common::lossless_payload(d, &info);
        assert!(
            payload.payload.head_record(19).is_some(),
            "near-lossless bounds"
        );
        assert!(payload.table.is_some());
        let full = common::decode_all(&file, &info);
        let bps = st.bytes();
        let span = info.width as usize * info.channels.count() * bps;
        for (a, b) in full.chunks(bps).zip(px.chunks(bps)) {
            let (a, b) = if bps == 1 {
                (a[0] as i64, b[0] as i64)
            } else {
                (
                    u16::from_ne_bytes([a[0], a[1]]) as i64,
                    u16::from_ne_bytes([b[0], b[1]]) as i64,
                )
            };
            assert!((a - b).abs() <= 2, "{st:?}: bound");
        }
        assert_eq!(full.len(), span * info.height as usize);
        check_regions(&file, true);
    }
}

/// `file` with `extra` bytes appended inside its (bounded) DATA payload.
fn with_data_slack(file: &[u8], extra: &[u8]) -> Vec<u8> {
    let (_, at, end) = chunks(file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let k = varint(&file[at + 4..]).1;
    let payload = &file[at + 4 + k..end];
    let mut out = file[..at + 4].to_vec();
    put_varint(&mut out, (payload.len() + extra.len()) as u64 + 1);
    out.extend_from_slice(payload);
    out.extend_from_slice(extra);
    out.extend_from_slice(&file[end..]);
    out
}

/// Bytes after the last record of a payload without a unit table are
/// corrupt for the region reader as for every full decoder: one-unit files
/// never have a table, and length-prefixed payloads have none.
#[test]
fn payloads_without_a_table_reject_trailing_data_bytes() {
    let info = ImageInfo::new(64, 64, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 111);
    let one = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(locator(&one).is_none(), "one unit: no table");
    #[cfg_attr(not(feature = "lab"), allow(unused_mut))]
    let mut files = vec![("one unit", one)];
    #[cfg(feature = "lab")]
    files.push((
        "length-prefixed",
        length_prefixed(|| {
            lossless_cfg(16, None)
                .encode_legacy_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap()
        }),
    ));
    for (what, file) in files {
        let bad = with_data_slack(&file, &[1, 2, 3]);
        assert_eq!(
            decode(&bad).err(),
            Some(Error::Corrupt("trailing DATA bytes")),
            "{what}"
        );
        assert_eq!(
            RegionReader::open(SliceReader::new(&bad)).err(),
            Some(Error::Corrupt("trailing DATA bytes")),
            "{what}"
        );
        assert!(
            RegionReader::open(SliceReader::new(&file)).is_ok(),
            "{what}"
        );
    }
}

/// A header that claims more units than the payload has bytes is corrupt
/// before the record walk reserves anything (each record takes at least
/// one byte), as in the full decoder.
#[test]
fn the_record_walk_checks_the_unit_count_first() {
    let info = ImageInfo::new(1, 16, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 112);
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&px, 1, &info), &Metadata::default())
        .unwrap();
    assert!(locator(&file).is_none(), "one unit: no table");
    let tall = common::with_dimensions(&file, 1, 16_000_000);
    let want = Some(Error::Corrupt("stripe count exceeds data"));
    assert_eq!(decode(&tall).err(), want);
    assert_eq!(RegionReader::open(SliceReader::new(&tall)).err(), want);
}

/// A lossless unit's decoding state is as wide as its column group, whatever
/// the region; it counts against the reader's `max_bytes` before it is
/// allocated.
#[test]
fn region_working_memory_is_charged_to_the_limits() {
    let info = ImageInfo::new(4096, 64, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 113);
    let file = lossless_cfg(16, None)
        .encode_legacy_samples(imij::ImageRef::new(&px, 4096, &info), &Metadata::default())
        .unwrap();
    let r = Rect::new(100, 20, 1, 1);
    let mut got = [0u8];
    let tight = {
        let mut limits = imij::Limits::DEFAULT;
        limits.max_bytes = 4096;
        limits
    };
    let mut rd = RegionReader::open_with_limits(SliceReader::new(&file), &tight).unwrap();
    rd.reset_read_stats();
    assert_eq!(
        rd.decode_region(r, &mut got, 1).err(),
        Some(Error::TooLarge)
    );
    // Refused before its records are fetched.
    assert_eq!(rd.read_stats().reads, 0);
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    rd.decode_region(r, &mut got, 1).unwrap();
    assert_eq!(got[0], px[20 * 4096 + 100]);
    // A small file whose header claims 2^27 columns: refused before the
    // 2^27-wide state is allocated.
    let small = ImageInfo::new(4096, 2, Channels::Gray, SampleType::U8);
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&pixels(&small, 114), 4096, &small),
            &Metadata::default(),
        )
        .unwrap();
    let wide = common::with_dimensions(&file, 1 << 27, 2);
    assert_eq!(decode(&wide).err(), Some(Error::TooLarge));
    let mut out = [0u8];
    assert_eq!(
        decode_region(&wide, Rect::new(5, 0, 1, 1), &mut out, 1).err(),
        Some(Error::TooLarge)
    );
}

/// The lossy counterpart (PR #11 review finding 6): a work item's
/// coefficients, reconstruction scratch, staged rows and nested DC and
/// alpha state count against `max_bytes` before any record is fetched, so
/// a 1x1 region of a small file whose header claims a huge width is refused
/// instead of allocating that scratch.
#[test]
fn lossy_region_working_memory_is_charged_to_the_limits() {
    let info = ImageInfo::new(256, 64, Channels::Rgba, SampleType::U8);
    let px = pixels(&info, 116);
    let file = imij::ImiEncoderConfig::lossy(80.0)
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let r = Rect::new(100, 20, 1, 1);
    let mut got = [0u8; 4];
    let tight = {
        let mut limits = imij::Limits::DEFAULT;
        limits.max_bytes = 64 * 1024;
        limits
    };
    let mut rd = RegionReader::open_with_limits(SliceReader::new(&file), &tight).unwrap();
    rd.reset_read_stats();
    assert_eq!(
        rd.decode_region(r, &mut got, 4).err(),
        Some(Error::TooLarge)
    );
    assert_eq!(rd.read_stats().reads, 0);
    let full = decode(&file).unwrap().1;
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    rd.decode_region(r, &mut got, 4).unwrap();
    assert_eq!(&got[..], &full[(20 * 256 + 100) * 4..][..4]);
    // The PR #11 review's case: the header claims 2^22 columns.
    let wide = common::with_dimensions(&file, 1 << 22, 64);
    assert_eq!(
        decode_region(&wide, Rect::new(5, 0, 1, 1), &mut got, 4).err(),
        Some(Error::TooLarge)
    );
}

/// A region's records are fetched whole (a unit decodes from its first
/// row): their bytes count against `max_bytes` before the fetch.
#[test]
fn region_records_are_charged_to_the_limits() {
    let info = ImageInfo::new(1024, 64, Channels::Gray, SampleType::U8);
    // Noise: the unit's record is larger than its decoding state.
    let mut x = 0x9e37_79b9u32;
    let px: Vec<u8> = (0..1024 * 64)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    let file = lossless_cfg(64, None)
        .encode_legacy_samples(imij::ImageRef::new(&px, 1024, &info), &Metadata::default())
        .unwrap();
    let r = Rect::new(100, 20, 1, 1);
    let mut got = [0u8];
    let open = |max_bytes| {
        let limits = {
            let mut limits = imij::Limits::DEFAULT;
            limits.max_bytes = max_bytes;
            limits.verify_checksum = false;
            limits
        };
        RegionReader::open_with_limits(SliceReader::new(&file), &limits).unwrap()
    };
    // The bytes a region fetches after the first: its records alone.
    let mut rd = open(u64::MAX);
    rd.decode_region(r, &mut got, 1).unwrap();
    rd.reset_read_stats();
    rd.decode_region(r, &mut got, 1).unwrap();
    let records = rd.read_stats().bytes;
    assert_eq!(got[0], px[20 * 1024 + 100]);
    let mut rd = open(records);
    rd.decode_region(r, &mut got, 1).unwrap();
    let mut rd = open(records - 1);
    rd.reset_read_stats();
    assert_eq!(
        rd.decode_region(r, &mut got, 1).err(),
        Some(Error::TooLarge)
    );
    assert_eq!(rd.read_stats().reads, 0);
}

/// The locator's DATA prefix is a claim: opening reads a bounded window of
/// DATA's head first and fetches more only when the head's own table length
/// agrees, so a forged prefix cannot turn opening into a whole-file read.
#[test]
fn a_locator_prefix_does_not_inflate_the_open_read() {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 115);
    let mut file = lossless_cfg(16, Some(64))
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    // An 8 MiB ancillary chunk after DATA.
    let big = vec![7u8; 8 << 20];
    file.extend_from_slice(b"zBIG");
    put_varint(&mut file, big.len() as u64 + 1);
    file.extend_from_slice(&big);
    let (prefix, lens) = locator(&file).unwrap();
    let rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(rd.used_locator());
    let honest = rd.read_stats().bytes;
    let mut recs: Vec<(u64, Vec<u8>)> =
        extensions(&file).into_iter().filter(|e| e.0 != 4).collect();
    recs.push((4, locator_record(prefix + (7 << 20), &lens)));
    let forged = with_extensions(&file, &recs);
    let rd = RegionReader::open(SliceReader::new(&forged)).unwrap();
    assert!(!rd.used_locator());
    let read = rd.read_stats().bytes;
    assert!(
        read < 4 * 65536,
        "forged prefix read {read} bytes (honest {honest})"
    );
    let mut got = vec![0u8; 30 * 20 * 3];
    let mut rd = rd;
    rd.decode_region(Rect::new(10, 10, 30, 20), &mut got, 90)
        .unwrap();
    assert_eq!(got, crop(&px, &info, Rect::new(10, 10, 30, 20)));
}

/// A head and table longer than the first window still open through the
/// locator: the head's table length matches the prefix, so the rest comes in
/// one more batch.
#[test]
fn a_long_unit_table_opens_through_the_locator() {
    let info = ImageInfo::new(2048, 4096, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 116);
    let file = lossless_cfg(1, Some(256))
        .encode_legacy_samples(imij::ImageRef::new(&px, 2048, &info), &Metadata::default())
        .unwrap();
    let (prefix, _) = locator(&file).unwrap();
    assert!(prefix > 65536, "prefix {prefix}");
    let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert!(rd.used_locator() && rd.has_unit_table());
    // Three batches, and one more for the record heads (the hint check,
    // SPEC 2.7).
    assert_eq!(rd.read_stats().batches, 4);
    let r = Rect::new(700, 3000, 64, 40);
    let mut got = vec![0u8; 64 * 40];
    rd.decode_region(r, &mut got, 64).unwrap();
    assert_eq!(got, crop(&px, &info, r));
}

/// Bytes at the end of a record that its decode never reads (SPEC 2.7):
/// every decoder ignores them in the rANS streams of lossless, DC and alpha
/// records and in a lossy record's compact rANS payload (which stays whole
/// 16-bit words), and refuses them in an exact record, which is parsed
/// whole before decoding. Every route gives the same outcome.
#[test]
fn record_slack_has_one_outcome_in_every_decoder() {
    let rgb = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let rgba = ImageInfo::new(256, 192, Channels::Rgba, SampleType::U8);
    let px = pixels(&rgb, 117);
    let pxa = pixels(&rgba, 118);
    let lossless = lossless_cfg(16, Some(64))
        .encode_legacy_samples(
            imij::ImageRef::new(&px, rgb.row_bytes(), &rgb),
            &Metadata::default(),
        )
        .unwrap();
    let lossy = |px: &[u8], info: &ImageInfo, exact: Option<f32>| {
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0).tune_lossy(|t| t.stripe_block_rows = 1);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = exact;
        });
        cfg.encode_legacy_samples(
            imij::ImageRef::new(px, info.row_bytes(), info),
            &Metadata::default(),
        )
        .unwrap()
    };
    const ODD: Error = Error::Corrupt("odd compact rANS payload");
    const EXACT: Error = Error::Corrupt("exact record");
    // (record, outcome with 7 slack bytes, with 8). Lossy with alpha: unit 0
    // is DC, alpha, lossy; the last record is a lossy record. With exact
    // blocks: DC, lossy, exact; the last record is an exact record.
    let lossy_alpha = lossy(&pxa, &rgba, None);
    let lossy_exact = lossy(&px, &rgb, Some(1000.0));
    let n = |file: &[u8]| {
        let h = imij::header(file).unwrap();
        let d = &file[common::data_payload(file)];
        common::payload(d, &h.info, h.coding == imij::Coding::Lossy)
            .records()
            .len()
    };
    let (nl, na, ne) = (n(&lossless), n(&lossy_alpha), n(&lossy_exact));
    // Per record: the outcome with 7 and with 8 bytes of slack.
    type Outcomes = Vec<(usize, Option<Error>, Option<Error>)>;
    let cases: [(&str, &[u8], &ImageInfo, Outcomes); 3] = [
        (
            "lossless",
            &lossless,
            &rgb,
            vec![(0, None, None), (1, None, None), (nl - 1, None, None)],
        ),
        (
            "lossy alpha",
            &lossy_alpha,
            &rgba,
            vec![
                (0, None, None),
                (1, None, None),
                (2, Some(ODD), None),
                (na - 1, Some(ODD), None),
            ],
        ),
        (
            "lossy exact",
            &lossy_exact,
            &rgb,
            vec![
                (0, None, None),
                (1, Some(ODD), None),
                (2, Some(EXACT), Some(EXACT)),
                (ne - 1, Some(EXACT), Some(EXACT)),
            ],
        ),
    ];
    for (what, file, info, records) in cases {
        let want = decode(file).unwrap().1;
        let rb = info.row_bytes();
        for (i, odd, even) in records {
            for (extra, err) in [(7, odd), (8, even)] {
                let slack = common::with_record_slack(file, i, &vec![0xa5; extra]);
                let label = format!("{what}, record {i}, {extra} bytes");
                let check = |got: Result<Vec<u8>, Error>, route: &str| match &err {
                    None => assert_eq!(got.as_ref(), Ok(&want), "{label}: {route}"),
                    Some(e) => assert_eq!(got.err().as_ref(), Some(e), "{label}: {route}"),
                };
                check(decode(&slack).map(|d| d.1), "decode");
                let mut out = vec![0u8; want.len()];
                check(
                    imij::decode_into_threads(&slack, &mut out, rb, 4).map(|_| out.clone()),
                    "threads",
                );
                let streamed = (|| {
                    let mut d = StreamDecoder::new(&slack)?;
                    let (mut all, mut row) = (Vec::new(), vec![0u8; rb]);
                    while d.next_row(&mut row)? {
                        all.extend_from_slice(&row);
                    }
                    Ok(all)
                })();
                check(streamed, "stream");
                let mut read = Vec::new();
                let r = imij::decode_reader(
                    std::io::Cursor::new(&slack),
                    &imij::Limits::DEFAULT,
                    |_, _, row| {
                        read.extend_from_slice(row);
                        Ok(())
                    },
                );
                check(r.map(|_| read), "reader");
                let whole = Rect::new(0, 0, info.width, info.height);
                let mut got = vec![0u8; want.len()];
                check(
                    decode_region(&slack, whole, &mut got, rb).map(|_| got),
                    "region",
                );
            }
        }
    }
}

/// A locator naming many chunks: their headers past the first window are
/// read as merged ranges (adjacent headers share one), and each chunk finds
/// its range directly, so checking the locator is linear in the chunk count
/// (review of PR #11, finding 4: 320,000 five-byte chunks took 13 s and
/// 320,003 ranges, the chunk walk 25 ranges).
#[test]
fn a_locator_of_many_chunks_is_checked_in_linear_time() {
    let info = ImageInfo::new(64, 48, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 119);
    let file = lossless_cfg(16, None)
        .encode_legacy_samples(imij::ImageRef::new(&px, 64, &info), &Metadata::default())
        .unwrap();
    let (prefix, lens) = locator(&file).unwrap();
    for n in [80_000usize, 320_000] {
        // n empty ancillary chunks before DATA, named by the locator.
        let (_, data_tag, _) = chunks(&file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
        let mut f = file[..data_tag].to_vec();
        for _ in 0..n {
            f.extend_from_slice(b"zzzz\x01");
        }
        f.extend_from_slice(&file[data_tag..]);
        let mut all = lens.clone();
        all.extend(std::iter::repeat_n(5, n));
        let mut recs: Vec<(u64, Vec<u8>)> =
            extensions(&f).into_iter().filter(|e| e.0 != 4).collect();
        recs.push((4, locator_record(prefix, &all)));
        let f = with_extensions(&f, &recs);
        assert_eq!(decode(&f).unwrap().1, px);
        let t = std::time::Instant::now();
        let mut rd = RegionReader::open(SliceReader::new(&f)).unwrap();
        let took = t.elapsed();
        assert!(rd.used_locator(), "{n}");
        let s = rd.read_stats();
        // The window, the rest of the header (the locator alone is n
        // varints, longer than the window), then one range for the chunk
        // headers and DATA's head, then the record heads (the hint check,
        // SPEC 2.7).
        assert!(s.batches <= 4, "{n}: {s:?}");
        eprintln!("{n} chunks under a locator: open {took:?}, {s:?}");
        assert!(took.as_secs_f64() < 5.0, "{n}: open took {took:?}");
        let mut got = vec![0u8; 64 * 48];
        rd.decode_region(Rect::new(0, 0, 64, 48), &mut got, 64)
            .unwrap();
        assert_eq!(got, px, "{n}");
    }
}

const MISMATCH: Error = Error::Corrupt("file checksum mismatch");

/// `rect` of `file` through a region reader opened with `limits`.
fn region_with(file: &[u8], rect: Rect, limits: &imij::Limits) -> Result<Vec<u8>, Error> {
    let mut rd = RegionReader::open_with_limits(SliceReader::new(file), limits)?;
    let bpp = rd.info().row_bytes() / rd.info().width as usize;
    let stride = rect.width() as usize * bpp;
    let mut out = vec![0u8; stride * rect.height() as usize];
    rd.decode_region(rect, &mut out, stride)?;
    Ok(out)
}

/// A source whose `n`-th batch fails with `Error::Read`.
struct FailingBatch<'a> {
    bytes: &'a [u8],
    batch: usize,
    fail: usize,
}

impl ReadAt for FailingBatch<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let a = offset as usize;
        out.copy_from_slice(&self.bytes[a..a + out.len()]);
        Ok(())
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.batch += 1;
        if self.batch == self.fail {
            return Err(Error::Read);
        }
        for (offset, out) in reqs.iter_mut() {
            self.read_exact_at(*offset, out)?;
        }
        Ok(())
    }
}

/// The pieces under what opening read (the header, the chunk headers, the
/// `DATA` head and table) are checked with the first region that succeeds:
/// a first region whose batch fails leaves them to the next (before, they
/// were dropped with the failed batch, and a later region decoded with
/// unchecked models).
#[test]
fn opening_bytes_are_checked_after_a_failed_batch() {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 3);
    let plain = lossless_cfg(16, None)
        .with_checksum(true)
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    // 1 KiB pieces: the head's pieces lie before the last stripe's.
    let base = common::checksum::strip(&plain);
    let file = common::checksum::with_table(&base, 10, 8, common::checksum::Place::BeforeData);
    let d = common::data_payload(&file);
    let lp = common::lossless_payload(&file[d.clone()], &info);
    let models = d.start + lp.payload.models.start..d.start + lp.payload.models.end;
    let last = Rect::new(0, 192, 300, 8);
    let mut checked = 0;
    for at in models {
        let mut bad = file.clone();
        bad[at] ^= 0x04;
        let src = FailingBatch {
            bytes: &bad,
            batch: 0,
            fail: 2,
        };
        // Opening parses the models without checking them; skip changes it
        // refuses outright.
        let Ok(mut rd) = RegionReader::open(src) else {
            continue;
        };
        let mut out = vec![0u8; 300 * 8 * 3];
        assert_eq!(rd.decode_region(last, &mut out, 900), Err(Error::Read));
        assert_eq!(
            rd.decode_region(last, &mut out, 900),
            Err(MISMATCH),
            "model byte at {at}"
        );
        checked += 1;
    }
    assert!(checked > 0);
}

/// The region reader verifies a file checksum (SPEC 1.4) like every other
/// decoder: every piece under a byte it uses is checked before it decodes,
/// and `Limits::verify_checksum` turns that off (review of PR #11, finding
/// 2: 150 of 2,963 DATA flips gave `Ok` with wrong pixels). The whole image
/// uses every DATA byte, so every flip a full decode reports is an error; a
/// part of it may decode only to the source's pixels. Unverified, a region
/// reads the file as a full decode without checksums does.
#[test]
fn regions_verify_the_file_checksum() {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 3);
    let file = lossless_cfg(16, None)
        .with_checksum(true)
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let on = imij::Limits::DEFAULT;
    let off = imij::Limits::DEFAULT.with_checksum_verification(false);
    let whole = Rect::new(0, 0, 300, 200);
    let part = Rect::new(100, 60, 50, 40);
    assert_eq!(region_with(&file, whole, &on).unwrap(), px);
    let mut flips = 0;
    for at in common::data_payload(&file).step_by(37) {
        let mut bad = file.clone();
        bad[at] ^= 0x10;
        assert_eq!(decode(&bad).err(), Some(MISMATCH), "flip at {at}");
        let got = region_with(&bad, whole, &on);
        assert!(
            got.is_err(),
            "flip at {at}: whole region {:?}",
            got.map(|_| ())
        );
        if let Ok(got) = region_with(&bad, part, &on) {
            assert_eq!(got, crop(&px, &info, part), "flip at {at}: part");
        }
        let full = imij::decode_with_limits(&bad, &off).map(|d| d.1);
        match (full, region_with(&bad, whole, &off)) {
            (Ok(f), Ok(r)) => assert_eq!(f, r, "flip at {at}, unverified"),
            (Err(_), Err(_)) => {}
            (f, r) => panic!(
                "flip at {at}, unverified: decode {:?}, region {:?}",
                f.map(|_| ()),
                r.map(|_| ())
            ),
        }
        flips += 1;
    }
    // One flip per 37 DATA bytes of a 62 KB payload.
    assert!(flips > 1600, "{flips}");
}

/// Verification costs no extra batch: the table, the pieces under the bytes
/// opening read and the region's records (rounded out to whole pieces) come
/// in the region's one batch. A table after `DATA` (a streaming writer's
/// placement) is found there; a header record without a table is corrupt
/// when verifying and ignored when not.
#[test]
fn region_checksums_cost_no_batch_and_follow_the_table() {
    let info = ImageInfo::new(640, 512, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 120);
    let rb = info.row_bytes();
    let cfg = lossless_cfg(16, Some(128));
    let plain = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &metadata())
        .unwrap();
    let file = cfg
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &metadata())
        .unwrap();
    let r = Rect::new(300, 400, 32, 16);
    let want = crop(&px, &info, r);
    let stats = |f: &[u8], limits: &imij::Limits| {
        let mut rd = RegionReader::open_with_limits(SliceReader::new(f), limits).unwrap();
        let open = rd.read_stats();
        rd.reset_read_stats();
        let mut out = vec![0u8; want.len()];
        rd.decode_region(r, &mut out, 96).unwrap();
        assert_eq!(out, want);
        (open, rd.read_stats(), rd.used_locator())
    };
    let off = imij::Limits::DEFAULT.with_checksum_verification(false);
    let (open_plain, region_plain, _) = stats(&plain, &imij::Limits::DEFAULT);
    let (open_on, region_on, located) = stats(&file, &imij::Limits::DEFAULT);
    let (open_off, region_off, _) = stats(&file, &off);
    eprintln!(
        "open {open_plain:?} / {open_on:?} / {open_off:?}; region {region_plain:?} / {region_on:?} / {region_off:?} (no checksum / verified / not verified)"
    );
    assert!(located);
    assert_eq!(open_on.batches, open_plain.batches);
    assert_eq!(
        (region_plain.batches, region_on.batches, region_off.batches),
        (1, 1, 1)
    );
    assert_eq!(open_off.batches, open_plain.batches);
    // Verified, the region's ranges grow to whole pieces (64 KiB here).
    assert!(
        region_on.bytes >= 65536 && region_off.bytes < 65536,
        "{region_on:?} {region_off:?}"
    );
    // The table moved to the end of the file: the covered bytes do not
    // change, the locator no longer matches, and the table is found after
    // DATA.
    let (_, csum_at, csum_end) = chunks(&file).into_iter().find(|c| &c.0 == b"cSUM").unwrap();
    let mut moved = file[..csum_at].to_vec();
    moved.extend_from_slice(&file[csum_end..]);
    moved.extend_from_slice(&file[csum_at..csum_end]);
    assert_eq!(decode(&moved).unwrap().1, px);
    let (_, _, located) = stats(&moved, &imij::Limits::DEFAULT);
    assert!(!located);
    let mut bad = moved.clone();
    bad[common::data_payload(&moved).start + 100] ^= 1;
    assert_eq!(decode(&bad).err(), Some(MISMATCH));
    assert_eq!(
        region_with(&bad, Rect::new(0, 0, 640, 512), &imij::Limits::DEFAULT).err(),
        Some(MISMATCH)
    );
    // The header record without its table.
    let mut missing = file[..csum_at].to_vec();
    missing.extend_from_slice(&file[csum_end..]);
    let missing_err = Some(Error::Corrupt("missing file checksum"));
    assert_eq!(decode(&missing).err(), missing_err);
    assert_eq!(
        region_with(&missing, r, &imij::Limits::DEFAULT).err(),
        missing_err
    );
    assert_eq!(region_with(&missing, r, &off).unwrap(), want);
}

/// A plane set built on a checksummed primary moves the primary's `cSUM`
/// chunk (now covering the whole set, so longer) and rewrites the primary's
/// DATA locator to name it at its new length (review of PR #11, finding 7:
/// the locator kept the primary's 18-byte table and was ignored).
#[test]
fn checksummed_plane_sets_keep_the_locator_in_step() {
    use imij::planes::{AttachmentDescriptor, PlaneSetBuilder};
    let info = ImageInfo::new(64, 48, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 21);
    let primary = lossless_cfg(16, None)
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&px, 64, &info), &Metadata::default())
        .unwrap();
    assert!(
        RegionReader::open(SliceReader::new(&primary))
            .unwrap()
            .used_locator()
    );
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let blob: Vec<u8> = (0..300_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    b.add_attachment(
        AttachmentDescriptor::new("blob", "application/octet-stream"),
        &blob,
    )
    .unwrap();
    let set = b.finish().unwrap();
    assert_eq!(decode(&set).unwrap().1, px);
    let (_, lens) = locator(&set).unwrap();
    let (_, a, e) = chunks(&set).into_iter().find(|c| &c.0 == b"cSUM").unwrap();
    assert_eq!(lens.last(), Some(&((e - a) as u64)));
    let mut rd = RegionReader::open(SliceReader::new(&set)).unwrap();
    assert!(rd.used_locator(), "{:?}", rd.read_stats());
    let mut out = vec![0u8; 64 * 48];
    rd.decode_region(Rect::new(0, 0, 64, 48), &mut out, 64)
        .unwrap();
    assert_eq!(out, px);
}

/// With a file checksum the first plan carries the checksum table and the
/// pieces under what opening read; the region then verifies in one batch.
#[test]
fn planned_ranges_cover_checksummed_regions() {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 92);
    let rb = info.row_bytes();
    let lossless = lossless_cfg(16, Some(64))
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
    let lossy = imij::ImiEncoderConfig::lossy(80.0)
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
    for file in [&lossless, &lossy] {
        let (_, full) = decode(file).unwrap();
        assert!(
            RegionReader::open(SliceReader::new(file))
                .unwrap()
                .has_file_checksum()
        );
        for r in [Rect::new(40, 30, 120, 90), Rect::new(0, 0, 300, 200)] {
            let want = crop(&full, &info, r);
            check_planned(file, r, &want, r.width() as usize * 3);
        }
    }
}

/// With `unit_ordinals` (head record 37, feature `unit_ordinals`, SPEC 2.7),
/// `RegionReader::open` skips the all-record-head preflight and opens in a
/// single 64 KiB read, while `decode_region` and walking decoders verify each
/// main record's 0-based ordinal locally.
#[test]
fn unit_ordinals_eliminate_open_head_reads_and_catch_forged_spans_locally() {
    let info = ImageInfo::new(640, 512, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 131);
    let rb = info.row_bytes();
    let base_ll = lossless_cfg(16, Some(128));
    let ord_ll = base_ll.clone().with_unit_ordinals(true);
    let file_base = base_ll
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
    let file_ord = ord_ll
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
    assert!(
        imij::header(&file_ord)
            .unwrap()
            .features
            .contains(imij::Feature::UnitOrdinals)
    );
    assert_eq!(decode(&file_ord).unwrap().1, px);

    let rd_base = RegionReader::open(SliceReader::new(&file_base)).unwrap();
    let rd_ord = RegionReader::open(SliceReader::new(&file_ord)).unwrap();
    // Without unit_ordinals, opening checks record heads beyond the first 64 KiB
    // window (2 batches); with unit_ordinals, open reads only the first window (1 batch, 1 read).
    assert_eq!(
        rd_base.read_stats().batches,
        2,
        "{:?}",
        rd_base.read_stats()
    );
    assert_eq!(rd_ord.read_stats().batches, 1, "{:?}", rd_ord.read_stats());
    assert_eq!(rd_ord.read_stats().reads, 1, "{:?}", rd_ord.read_stats());

    let lossy_cfg = imij::ImiEncoderConfig::lossy(80.0)
        .tune_lossy(|t| t.stripe_block_rows = 2)
        .with_unit_ordinals(true);
    let lossy_file = lossy_cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &Metadata::default())
        .unwrap();
    let (_, lossy_full) = decode(&lossy_file).unwrap();
    for (f, full) in [(&file_ord, &px), (&lossy_file, &lossy_full)] {
        let mut stream_rows = Vec::new();
        imij::decode_reader(&f[..], &imij::Limits::DEFAULT, |_, _, row| {
            stream_rows.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(&stream_rows, full);
        let mut rd = RegionReader::open(SliceReader::new(f)).unwrap();
        for r in [
            Rect::new(0, 0, 64, 32),
            Rect::new(150, 90, 210, 110),
            Rect::new(500, 380, 140, 132),
        ] {
            let want = crop(full, &info, r);
            let mut got = vec![0u8; want.len()];
            rd.decode_region(r, &mut got, r.width() as usize * 3)
                .unwrap();
            assert_eq!(got, want);
        }
        #[cfg(feature = "lab")]
        {
            let tr = imij::lab::codestream_trace(f);
            assert!(
                tr.contains("\"problems\":[],\"unattributed\":[]"),
                "trace checks failed: {tr}"
            );
            assert!(tr.contains("\"unit record ordinal\""));
        }
    }

    // Forged unit table hint on a unit_ordinals file: [l, l, l, l] -> [2l, l, 2, l - 2].
    // Unit 1's span points at Unit 2's record, whose embedded ordinal is 2 != 1.
    let small_info = ImageInfo::new(32, 64, Channels::Gray, SampleType::U8);
    let even: Vec<u8> = (0..2048)
        .map(|x| ((x / 512) * 50 + (x % 32)) as u8)
        .collect();
    let mut cfg = imij::ImiEncoderConfig::default().with_unit_ordinals(true);
    cfg = cfg.tune_lossless(|t| {
        t.stripe_rows = Some(16);
    });
    cfg = cfg.tune_lossless(|t| {
        t.palette = false;
    });
    let small_file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&even, 32, &small_info),
            &Metadata::default(),
        )
        .unwrap();
    let d = common::data_payload(&small_file);
    let t = common::payload(&small_file[d.clone()], &small_info, false)
        .table()
        .expect("a unit table hint");
    let (t0, t1) = (d.start + t.start, d.start + t.end);
    let mut forged = small_file.clone();
    forged[t0..t1].copy_from_slice(&forge_shift(&small_file[t0..t1]));
    assert!(decode(&forged).is_err());
    // RegionReader::open succeeds without reading record heads, and decoding
    // unit 1 catches the wrong ordinal locally instead of returning unit 2's pixels.
    let mut rd = RegionReader::open(SliceReader::new(&forged)).unwrap();
    let mut out = vec![0u8; 32 * 16];
    assert_eq!(
        rd.decode_region(Rect::new(0, 16, 32, 16), &mut out, 32),
        Err(Error::Corrupt("unit record ordinal"))
    );
}

/// A source that records the length of every range requested of it.
struct Recording<'a> {
    bytes: &'a [u8],
    lens: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
}

impl ReadAt for Recording<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.lens.borrow_mut().push(out.len());
        let a = offset as usize;
        out.copy_from_slice(&self.bytes[a..a + out.len()]);
        Ok(())
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        for (offset, out) in reqs.iter_mut() {
            self.read_exact_at(*offset, out)?;
        }
        Ok(())
    }
}

/// A region's byte budget (`Limits::max_bytes`) covers its whole fetch
/// plan, checksum pieces and table included (review 2026-10-11: the round
/// out to whole pieces was added after the budget check, so a small region
/// of a file with large pieces fetched and allocated whole pieces). Under
/// every budget no request of a region is larger than the budget: pieces
/// that do not fit are hashed through a bounded buffer, and a region whose
/// records and table do not fit is `TooLarge`. Verification stays on: a
/// damaged byte far from the region still fails it.
#[test]
fn checksum_pieces_stay_inside_the_region_budget() {
    let info = ImageInfo::new(512, 256, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 31);
    let cfg = {
        let mut c = lossless_cfg(16, Some(64));
        c = c.with_checksum(true);
        c
    };
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let full = decode(&file).unwrap().1;
    let base = common::checksum::strip(&file);
    let place = common::checksum::Place::BeforeData;
    for k in [10u8, 16, 30] {
        let f = common::checksum::with_table(&base, k, 8, place);
        assert!(f.len() > 100_000, "{}", f.len());
        for rect in [Rect::new(100, 100, 1, 1), Rect::new(64, 32, 64, 16)] {
            let mut ok = 0;
            for budget in [4u64 << 10, 16 << 10, 64 << 10, 256 << 10, 1 << 20] {
                let lens = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                let src = Recording {
                    bytes: &f,
                    lens: lens.clone(),
                };
                let limits = {
                    let mut l = Limits::DEFAULT;
                    l.max_bytes = budget;
                    l
                };
                let Ok(mut rd) = RegionReader::open_with_limits(src, &limits) else {
                    continue;
                };
                lens.borrow_mut().clear();
                let mut out = vec![0u8; rect.width() as usize * rect.height() as usize * 3];
                let r = rd.decode_region(rect, &mut out, rect.width() as usize * 3);
                let largest = lens.borrow().iter().copied().max().unwrap_or(0);
                assert!(
                    largest as u64 <= budget,
                    "k {k}, budget {budget}: a request of {largest} bytes"
                );
                match r {
                    Ok(()) => {
                        assert_eq!(out, crop(&full, &info, rect), "k {k}, budget {budget}");
                        ok += 1;
                    }
                    Err(Error::TooLarge) => {}
                    Err(e) => panic!("k {k}, budget {budget}: {e:?}"),
                }
            }
            assert!(ok >= 2, "k {k} {rect:?}: {ok} budgets decoded");
        }
        // A damaged byte near the end of the file, far from the region,
        // under the smallest budget that decodes.
        let mut bad = f.clone();
        let at = bad.len() - 100;
        bad[at] ^= 1;
        let limits = {
            let mut l = Limits::DEFAULT;
            l.max_bytes = 64 << 10;
            l
        };
        let mut rd = RegionReader::open_with_limits(SliceReader::new(&bad), &limits).unwrap();
        let mut out = vec![0u8; 3];
        let r = rd.decode_region(Rect::new(100, 100, 1, 1), &mut out, 3);
        if k == 30 {
            assert_eq!(r, Err(Error::Corrupt("file checksum mismatch")), "k {k}");
        }
    }
}

/// Opening checks the unit table hint against every main record's head
/// (SPEC 2.7): one read of up to 20 bytes per record past the bytes in hand,
/// in batches of 65,536, charged to `Limits::max_bytes` before each batch,
/// so a caller bounds the open's work.
#[test]
fn opening_checks_the_hint_in_bounded_batches() {
    let info = ImageInfo::new(128, 3000, Channels::Gray, SampleType::U8);
    let px = pixels(&info, 5);
    let cfg = lossless_cfg(1, None);
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    // 3,000 one-row stripes → many records past the first 64 KiB window.
    assert!(file.len() > 2 * 65536, "{}", file.len());
    let r = Rect::new(3, 2900, 40, 50);
    let want = crop(&px, &info, r);
    // Opens correctly under a generous budget.
    let src = Counting {
        bytes: &file,
        batches: Vec::new(),
    };
    let mut rd = RegionReader::open(src).unwrap();
    let opened = rd.read_stats().batches;
    let mut out = vec![0u8; 40 * 50];
    rd.decode_region(r, &mut out, 40).unwrap();
    assert_eq!(out, want);
    // Opening batches: never one per record.
    assert!(opened <= 4, "opened in {opened} batches");
    // The heads past the first 64 KiB (most of 3,000 records of up to 20
    // bytes each) do not fit a 16 KiB byte limit; they fit 1 MiB.
    let mut small = Limits::DEFAULT;
    small.max_bytes = 16 << 10;
    assert!(matches!(
        RegionReader::open_with_limits(SliceReader::new(&file), &small),
        Err(Error::TooLarge)
    ));
    let mut large = Limits::DEFAULT;
    large.max_bytes = 1 << 20;
    assert!(RegionReader::open_with_limits(SliceReader::new(&file), &large).is_ok());
}
