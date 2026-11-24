//! Piecewise `cSUM` file checksum (SPEC 1.4), writers: files carry a table
//! only when asked, every writer (previews, frames, pages, plane sets,
//! metadata remux) puts one outer table over nested files, and the table
//! matches an independent XXH3 (xxhash-rust).
// Literal V2/V3 framing controls use the retained compatibility writers.

mod common;
use common::checksum::*;

// ---- writers ----------------------------------------------------------------

#[test]
fn writers_add_a_header_record_and_a_table_before_data() {
    for (name, plain, with) in files() {
        assert!(find(&plain, b"cSUM").is_none(), "{name}");
        // Without the option nothing changes; with it, the file is the plain
        // one plus the header record [2, 1, k] and the table before DATA.
        assert_eq!(strip_all(&with), plain, "{name}");
        let (a, b) = csum(&with);
        assert_eq!(
            b,
            find(&with, b"DATA").unwrap().0,
            "{name}: table before DATA"
        );
        assert_eq!(with[record_at(&with)], 16, "{name}: one 64 KiB piece");
        assert_eq!(with.len() - plain.len(), 4 + (b - a), "{name}");
        // The reference writer builds the same bytes.
        assert_eq!(rebuilt(&with), with, "{name}");
        let (_, px) = decode(&plain).unwrap();
        accepted(&plain, &px, name);
        for (what, f) in variants(&with) {
            accepted(&f, &px, &format!("{name} {what}"));
            accepted_on(&OFF_PATHS, &f, &px, &format!("{name} {what}, off"));
        }
    }
}

#[test]
fn piece_size_follows_the_file_size() {
    // One piece up to 64 KiB, at most 16 pieces of 64 KiB to 1 MiB up to
    // 16 MiB, 1 MiB pieces beyond (benchmarks/csum_pieces_2026-10-07).
    for (w, h) in [(64, 64), (256, 256), (512, 512)] {
        let f = lossless_sized(w, h, true);
        let len = strip(&f).len() as u64;
        let k = f[record_at(&f)];
        let expect = if len <= 1 << 16 {
            16
        } else {
            (64 - (len.div_ceil(16) - 1).leading_zeros()).clamp(16, 20) as u8
        };
        assert_eq!(k, expect, "{w}x{h}: {len} bytes");
        assert_eq!(rebuilt(&f), f);
    }
}

#[test]
fn reference_digests_of_known_inputs() {
    // The xxHash reference value for "hello world", as zenhash's README
    // states it.
    assert_eq!(
        xxhash_rust::xxh3::xxh3_64(b"hello world"),
        0xD447_B1EA_40E6_988B
    );
}

#[test]
fn metadata_survives_and_the_chunk_is_not_returned_as_metadata() {
    let gain = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(
                &[7; 12],
                6,
                &ImageInfo::new(2, 2, Channels::Rgb, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap();
    let mut params = GainMapParams::default();
    for c in &mut params.channels {
        c.min = 0.0;
        c.max = 2.0;
    }
    params.alternate_hdr_headroom = 2.0;
    let mut m = meta();
    m.xmp = Some(b"<x:xmpmeta/>".to_vec());
    m.mastering_display = Some([3; 24]);
    m.content_light_level = Some([0, 15, 66, 64, 0, 7, 161, 32]);
    m.orientation = 6;
    m.gain_map = Some(GainMap {
        params,
        alt_cicp: None,
        alt_icc: None,
        image: gain,
    });
    let (info, px) = image(9, 5, Channels::Rgb);
    let f = imij::ImiEncoderConfig::default()
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &m)
        .unwrap();
    for (what, f) in variants(&f) {
        let (_, got) = probe(&f).unwrap();
        assert_eq!(got, m, "{what}");
        let mut out = Vec::new();
        let (_, streamed) = decode_reader(f.as_slice(), &Limits::DEFAULT, |i, y, r| {
            rows_into(i, &mut out, y, r)
        })
        .unwrap();
        assert_eq!(streamed, m, "{what}");
        let (_, unchecked) = decode_reader(f.as_slice(), &OFF, |_, _, _| Ok(())).unwrap();
        assert_eq!(unchecked, m, "{what}");
        // The gain map image is nested: covered by the outer table only.
        assert!(find(&got.gain_map.unwrap().image, b"cSUM").is_none());
    }
}

#[test]
fn applications_cannot_write_the_tag_as_their_own_chunk() {
    let (info, px) = image(4, 4, Channels::Rgb);
    let m = Metadata {
        chunks: vec![Chunk {
            tag: *b"cSUM",
            data: vec![0; 9],
        }],
        ..Default::default()
    };
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &m),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn previews_are_nested_and_covered_by_the_outer_table() {
    let (info, px) = image(16, 12, Channels::Rgb);
    let (pinfo, ppx) = image(64, 48, Channels::Rgb);
    let p = [ImageRef {
        pixels: &ppx,
        stride: pinfo.row_bytes(),
        info: &pinfo,
    }];
    let encode_with = |checksum| {
        let cfg = imij::ImiEncoderConfig::default().with_checksum(checksum);
        cfg.encode_samples_with_previews(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
            &p,
        )
        .unwrap()
    };
    let (plain, f) = (encode_with(false), encode_with(true));
    assert_eq!(strip_all(&f), plain);
    accepted(&f, &px, "previews");
    let nested = previews(&f).unwrap();
    assert_eq!(nested.len(), 1);
    assert!(find(nested[0], b"cSUM").is_none());
    assert_eq!(decode(nested[0]).unwrap().1, ppx);
    // A changed preview byte fails the outer file. With 1 KiB pieces and the
    // table before the previews, `previews` checks the pieces it reads.
    let pr = find(&f, b"pRVW").unwrap().1;
    let mut bad = f.clone();
    bad[pr.start + 100] ^= 0x40;
    assert_eq!(decode(&bad).err(), Some(MISMATCH));
    let base = strip(&f);
    let early = with_table(&base, 10, 8, Place::Chunk(0));
    let pr = find(&early, b"pRVW").unwrap().1;
    let mut bad = early.clone();
    bad[pr.start + 100] ^= 0x40;
    assert!(previews(&early).is_ok());
    assert_eq!(previews(&bad).err(), Some(MISMATCH));
    // `header` reads a prefix and checks nothing.
    assert_eq!(header(&bad).unwrap(), header(&early).unwrap());
}

fn frames(w: u32, h: u32) -> Vec<Vec<u8>> {
    let (info, base) = image(w, h, Channels::Rgb);
    (0..3)
        .map(|k| {
            let mut f = base.clone();
            for y in 2..5 {
                for x in 3..7 {
                    f[y * info.row_bytes() + x * 3] = (40 * k + y + x) as u8;
                }
            }
            f
        })
        .collect()
}

fn animation(coding: ImiEncoderConfig) -> Vec<u8> {
    let info = ImageInfo::new(12, 9, Channels::Rgb, SampleType::U8);
    let mut e = FrameEncoder::animation(info.clone(), meta(), FrameConfig::new(coding));
    for f in frames(12, 9) {
        e.push_frame(&f, info.row_bytes(), FrameDuration::from_millis(40))
            .unwrap();
    }
    e.finish_legacy().unwrap()
}

fn all_frames(f: &[u8], limits: &Limits) -> Result<Vec<Vec<u8>>, Error> {
    let mut d = FrameDecoder::with_limits(f, limits)?;
    let rb = d.info().row_bytes();
    let mut canvas = vec![0u8; d.info().checked_buffer_len().unwrap()];
    let mut out = Vec::new();
    while d.next_frame()?.is_some() {
        d.decode_into(&mut canvas, rb, 2)?;
        out.push(canvas.clone());
    }
    Ok(out)
}

#[test]
fn animations_and_pages_carry_one_outer_table() {
    let codings = [
        (
            imij::ImiEncoderConfig::default(),
            imij::ImiEncoderConfig::default().with_checksum(true),
        ),
        (
            imij::ImiEncoderConfig::lossy(85.0),
            imij::ImiEncoderConfig::lossy(85.0).with_checksum(true),
        ),
    ];
    for (off, on) in codings {
        let plain = animation(off);
        let f = animation(on);
        assert_eq!(strip_all(&f), plain);
        let expect = all_frames(&plain, &Limits::DEFAULT).unwrap();
        for (what, v) in variants(&f) {
            assert_eq!(all_frames(&v, &Limits::DEFAULT).unwrap(), expect, "{what}");
            accepted(&v, &expect[0], &what);
            let fr: Vec<_> = chunks(&v).into_iter().filter(|c| &c.1 == b"fRAM").collect();
            assert_eq!(fr.len(), 2);
            for (_, _, r) in &fr {
                let mut at = r.start;
                for _ in 0..4 {
                    get_varint(&v, &mut at);
                }
                let nested = &v[at + 1..r.end];
                assert!(nested.is_empty() || find(nested, b"cSUM").is_none());
            }
            // A changed byte of frame 1's image is caught by the outer table;
            // with verification off it decodes as without a checksum (seed
            // tables make this byte matter to a lossy frame).
            let mut bad = v.clone();
            bad[fr[1].2.end - 1] ^= 1;
            assert_eq!(FrameDecoder::new(&bad).err(), Some(MISMATCH), "{what}");
            assert_eq!(
                all_frames(&bad, &OFF),
                all_frames(&strip_all(&bad), &Limits::DEFAULT),
                "{what}"
            );
            // A changed frame duration (after x0, y0) is caught too, and with
            // verification off every frame still decodes.
            let mut at = fr[1].2.start;
            get_varint(&v, &mut at);
            get_varint(&v, &mut at);
            let mut slow = v.clone();
            slow[at] ^= 2;
            assert_eq!(FrameDecoder::new(&slow).err(), Some(MISMATCH), "{what}");
            assert_eq!(all_frames(&slow, &OFF).map(|f| f.len()), Ok(3), "{what}");
        }
    }
    let mut pages = Vec::new();
    for cfg in [
        imij::ImiEncoderConfig::default(),
        imij::ImiEncoderConfig::default().with_checksum(true),
    ] {
        let (i0, p0) = image(10, 6, Channels::Gray);
        let (i1, p1) = image(7, 11, Channels::Gray);
        let mut e = FrameEncoder::pages(i0.clone(), Metadata::default(), FrameConfig::new(cfg));
        e.push_page(&p0, i0.row_bytes(), &i0).unwrap();
        e.push_page(&p1, i1.row_bytes(), &i1).unwrap();
        pages.push(e.finish_legacy().unwrap());
    }
    assert_eq!(strip_all(&pages[1]), pages[0]);
    let mut d = FrameDecoder::new(&pages[1]).unwrap();
    let mut n = 0;
    while let Some(h) = d.next_frame().unwrap() {
        let mut out = vec![0u8; (h.width * h.height) as usize];
        d.decode_into(&mut out, h.width as usize, 1).unwrap();
        n += 1;
    }
    assert_eq!(n, 2);
}

#[test]
fn metadata_remux_recomputes_the_table() {
    let (info, px) = image(9, 9, Channels::Rgb);
    let mut m = meta();
    m.orientation = 1;
    m.icc = Some(b"acspmntrRGB XYZ rXYZgXYZbXYZrTRCgTRCbTRC".repeat(64));
    m.xmp = Some(b"<rdf:Description xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"/>".repeat(40));
    let plain = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &m)
        .unwrap();
    let packed_plain = imij::compress_metadata(&plain, 1 << 20).unwrap();
    assert!(packed_plain.len() < plain.len());
    let f = imij::ImiEncoderConfig::default()
        .with_checksum(true)
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &m)
        .unwrap();
    for (what, v) in variants(&f) {
        let packed = imij::compress_metadata(&v, 1 << 20).unwrap();
        assert_eq!(strip_all(&packed), packed_plain, "{what}");
        assert_eq!(rebuilt(&packed), packed, "{what}");
        accepted(&packed, &px, &what);
        assert_eq!(probe(&packed).unwrap().1, m);
        let mut bad = v.clone();
        bad[30] ^= 1;
        assert!(imij::compress_metadata(&bad, 1 << 20).is_err(), "{what}");
    }
    let mut unknown = strip(&f);
    let rec = record_at(&unknown) + 1;
    unknown[rec] = 7;
    let data = find(&unknown, b"DATA").unwrap().0;
    unknown.splice(data..data, b"cSUM\x05\x07\x01\x02\x03".iter().copied());
    assert_eq!(
        imij::compress_metadata(&unknown, 1 << 20).err(),
        Some(Error::Unsupported("unknown file checksum algorithm"))
    );
}

/// Incompressible bytes (xorshift), for metadata of an exact size.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// `f()` on another thread, or None when it has not returned after `secs`
/// (the thread is left running).
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs)).ok()
}

/// Covered length of a checksummed file (all but its cSUM chunk).
fn covered_len(f: &[u8]) -> usize {
    let (a, b) = csum(f);
    f.len() - (b - a)
}

/// The piece size grows with the file at 1, 2, 4 and 8 MiB. Writers settle
/// the DATA locator (which names the cSUM chunk) around the chunk's length:
/// with the piece size chosen from the length on every pass, a file whose
/// covered length sat on such a boundary flipped between two piece counts,
/// and `encode` and `compress_metadata` never returned (review of PR #11,
/// finding 1). Metadata sizes around each boundary, encoded and remuxed:
/// every call returns, the files verify, and the sizes cross the boundary.
#[test]
fn checksum_piece_size_boundaries_settle() {
    let info = ImageInfo::new(256, 64, Channels::Gray, SampleType::U8);
    let px: Vec<u8> = (0..256 * 64u32).map(|i| (i * 7 % 251) as u8).collect();
    // Four units: a unit table and a DATA locator.
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg
    };
    // The bulk is an ancillary vendor chunk: the remux keeps it as it is
    // (packable ICC, Exif and XMP are capped at the 1 MiB block limit).
    let bulk = |n: usize| Chunk {
        tag: *b"vnDr",
        data: noise(n, 99),
    };
    let bulk_only = |n: usize| Metadata {
        chunks: vec![bulk(n)],
        ..Default::default()
    };
    let with_xmp = |n: usize| Metadata {
        xmp: Some(b"<rdf:li>region</rdf:li>".repeat(4000)),
        chunks: vec![bulk(n)],
        ..Default::default()
    };
    let mut hangs = Vec::new();
    for log2 in 20..=23u32 {
        let boundary = 1usize << log2;
        let n0 = boundary - 12_000;
        // encode: the checksum adds its 3-byte header record and a locator
        // entry (1 byte) to the plain file's covered length.
        let plain = cfg
            .encode_legacy_samples(imij::ImageRef::new(&px, 256, &info), &bulk_only(n0))
            .unwrap()
            .len();
        let target = boundary + n0 - (plain + 4);
        let mut covered = Vec::new();
        for n in target - 8..=target + 8 {
            let (px, info, cfg, m) = (
                px.clone(),
                info.clone(),
                cfg.clone().with_checksum(true),
                bulk_only(n),
            );
            match within(20, move || {
                cfg.encode_legacy_samples(imij::ImageRef::new(&px, 256, &info), &m)
                    .unwrap()
            }) {
                Some(f) => {
                    assert!(decode(&f).is_ok(), "encode, bulk {n}");
                    covered.push(covered_len(&f));
                }
                None => hangs.push(format!("encode, 2^{log2}, bulk {n}")),
            }
        }
        if let (Some(lo), Some(hi)) = (covered.iter().min(), covered.iter().max()) {
            assert!(
                *lo <= boundary && *hi > boundary,
                "encode 2^{log2}: {lo}..{hi}"
            );
        }
        // compress_metadata of a valid checksummed file: the remux packs the
        // XMP and seals the result.
        let remux_plain = |n: usize| {
            let f = cfg
                .encode_legacy_samples(imij::ImageRef::new(&px, 256, &info), &with_xmp(n))
                .unwrap();
            imij::compress_metadata(&f, 1 << 20).unwrap().len()
        };
        let target = boundary + n0 - (remux_plain(n0) + 4);
        let mut covered = Vec::new();
        for n in target - 8..=target + 8 {
            let src = cfg
                .clone()
                .with_checksum(true)
                .encode_legacy_samples(imij::ImageRef::new(&px, 256, &info), &with_xmp(n))
                .unwrap();
            match within(20, move || imij::compress_metadata(&src, 1 << 20).unwrap()) {
                Some(f) => {
                    assert!(decode(&f).is_ok(), "remux, bulk {n}");
                    covered.push(covered_len(&f));
                }
                None => hangs.push(format!("compress_metadata, 2^{log2}, bulk {n}")),
            }
        }
        if let (Some(lo), Some(hi)) = (covered.iter().min(), covered.iter().max()) {
            assert!(
                *lo <= boundary && *hi > boundary,
                "remux 2^{log2}: {lo}..{hi}"
            );
        }
    }
    assert!(hangs.is_empty(), "no result after 20 s: {hangs:?}");
}

/// Quality targets (`LossyQuality`) resolve before coding; the checksum is
/// the same outer table: the target's plain file plus the record and table.
#[test]
fn quality_targets_take_a_checksum_like_a_scale() {
    use imij::LossyQuality;
    let (info, px) = image(40, 24, Channels::Rgb);
    let rb = info.row_bytes();
    for target in [
        LossyQuality::Ssim2(80.0),
        LossyQuality::JpegQuality(75.0),
        LossyQuality::JxlDistance(1.5),
        LossyQuality::Scale(60.0),
    ] {
        let cfg = imij::ImiEncoderConfig::lossy(90.0).with_quality(target);
        let plain = cfg
            .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta())
            .unwrap();
        let f = cfg
            .with_checksum(true)
            .encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta())
            .unwrap();
        assert_eq!(strip_all(&f), plain, "{target:?}");
        let (_, pixels) = decode(&plain).unwrap();
        accepted(&f, &pixels, &format!("{target:?}"));
        accepted_on(&OFF_PATHS, &f, &pixels, &format!("{target:?} off"));
    }
}

// ---- science records --------------------------------------------------------

fn calibration(unit: &str) -> imij::science::ScienceRecords {
    let mut c = imij::science::ChannelCalibration::default();
    c.slope = 0.5;
    c.offset = -12.0;
    c.unit = Some(unit.into());
    let mut r = imij::science::ScienceRecords::default();
    r.calibration.push(c);
    r
}

/// Science records (SPEC 1.5) are ordinary ancillary chunks: the table covers
/// them like any other chunk, and remux keeps both.
#[test]
fn science_records_are_covered_like_any_chunk() {
    use imij::science::ScienceRecords;
    let (info, px) = image(29, 17, Channels::Gray);
    let records = calibration("kelvin");
    let mut meta = Metadata::default();
    records.write_metadata(&info, &mut meta).unwrap();
    let rb = info.row_bytes();
    let lossless = |checksum| {
        let cfg = imij::ImiEncoderConfig::default().with_checksum(checksum);
        cfg.encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta)
            .unwrap()
    };
    let lossy = |checksum| {
        let cfg = imij::ImiEncoderConfig::lossy(80.0).with_checksum(checksum);
        cfg.encode_legacy_samples(imij::ImageRef::new(&px, rb, &info), &meta)
            .unwrap()
    };
    let files = [
        ("lossless", lossless(false), lossless(true)),
        ("lossy", lossy(false), lossy(true)),
    ];
    for (name, plain, f) in files {
        assert_eq!(strip_all(&f), plain, "{name}");
        let (_, pixels) = decode(&plain).unwrap();
        accepted(&f, &pixels, name);
        let (a, b) = csum(&f);
        assert_eq!(
            &f[a..b],
            table_chunk(&strip(&f), f[record_at(&f)], 16),
            "{name}"
        );
        let (pinfo, pmeta) = probe(&f).unwrap();
        assert_eq!(
            ScienceRecords::from_metadata(&pinfo, &pmeta).unwrap(),
            records
        );
        // A changed record byte: every verifying path fails; decoders without
        // verification still decode the pixels (they never read the records).
        let (_, calb) = find(&f, b"cALB").unwrap();
        let mut bad = f.clone();
        bad[calb.start + calb.len() / 2] ^= 1;
        rejected(&bad, Some(&MISMATCH), name);
        accepted_on(&OFF_PATHS, &bad, &pixels, name);
        // Remux rewrites the table over the same records.
        let packed = imij::compress_metadata(&f, 1 << 20).unwrap();
        accepted(&packed, &pixels, name);
        let (pa, pb) = csum(&packed);
        assert_eq!(
            &packed[pa..pb],
            table_chunk(&strip(&packed), packed[record_at(&packed)], 16),
            "{name}"
        );
        let (pinfo, pmeta) = probe(&packed).unwrap();
        assert_eq!(
            ScienceRecords::from_metadata(&pinfo, &pmeta).unwrap(),
            records
        );
    }
}

/// A plane set's science attachment is a plane record like any other: the
/// table covers it, so whole-file decoders and a round-out read catch a
/// changed byte.
#[test]
fn plane_set_science_attachment_is_covered() {
    let mut b = PlaneSetBuilder::new(&primary(true)).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("depth", SampleType::U16).with_role(PlaneRole::Depth))
        .unwrap();
    let pixels: Vec<u8> = (0u16..64).flat_map(|n| (n * 991).to_ne_bytes()).collect();
    let coding = imij::ImiEncoderConfig::default();
    b.add_level(id, LevelDescriptor::new(8, 8, 4, 4), &pixels, 16, coding)
        .unwrap();
    let records = calibration("meter");
    b.set_science(Some(id), records.clone()).unwrap();
    let f = b.finish().unwrap();
    // The default checks every piece a read overlaps; the partial-piece
    // policy only those wholly inside it.
    let full = PlaneSetLimits::default();
    let partial = full.with_checksum_round_out(false);
    for limits in [partial, full] {
        let mut r = plane_reader(&f, limits).unwrap();
        assert_eq!(r.science(id).unwrap(), records);
    }
    let (_, primary_px) = decode(&primary(false)).unwrap();
    assert_eq!(decode(&f).unwrap().1, primary_px);
    // The builder appends the records' attachment as the last plane record.
    let last = chunks(&f).into_iter().rfind(|c| &c.1 == b"pDAT").unwrap().2;
    let mut bad = f.clone();
    bad[last.start + last.len() / 2] ^= 1;
    assert_eq!(decode(&bad).err(), Some(MISMATCH));
    let science = |limits| plane_reader(&bad, limits).and_then(|mut r| r.science(id));
    assert_eq!(science(full).err(), Some(MISMATCH));
    let off = full.with_checksum_verification(false);
    assert_ne!(science(off).err(), Some(MISMATCH));
}

// ---- named planes -----------------------------------------------------------------

fn primary(checksum: bool) -> Vec<u8> {
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default().with_effort(0.0);
        cfg = cfg.with_checksum(checksum);
        cfg
    };
    let (info, px) = image(6, 5, Channels::Gray);
    cfg.encode_legacy_samples(
        imij::ImageRef::new(&px, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap()
}

type PlaneFile = (Vec<u8>, PlaneId, AttachmentId, Vec<u8>);

/// A depth plane and a 300 KB attachment (many whole pieces).
fn plane_file(primary: &[u8]) -> Result<PlaneFile, Error> {
    let mut b = PlaneSetBuilder::new(primary)?;
    let id =
        b.add_plane(PlaneDescriptor::new("depth", SampleType::U16).with_role(PlaneRole::Depth))?;
    let pixels: Vec<u8> = (0u16..64).flat_map(|n| (n * 991).to_ne_bytes()).collect();
    let coding = imij::ImiEncoderConfig::default().with_checksum(true);
    b.add_level(id, LevelDescriptor::new(8, 8, 4, 4), &pixels, 16, coding)?;
    let blob: Vec<u8> = (0..300_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let a = b.add_attachment(
        AttachmentDescriptor::new("blob", "application/octet-stream"),
        &blob,
    )?;
    Ok((b.finish()?, id, a, pixels))
}

fn plane_reader(
    file: &[u8],
    limits: PlaneSetLimits,
) -> Result<PlaneSetReader<SliceReader<'_>>, Error> {
    PlaneSetReader::open(SliceReader::new(file), limits)
}

fn read_region(file: &[u8], id: PlaneId, limits: PlaneSetLimits) -> Result<Vec<u8>, Error> {
    let mut r = plane_reader(file, limits)?;
    let mut out = vec![0u8; 128];
    r.decode_region(id, 0, Rect::new(0, 0, 8, 8), &mut out, 16)?;
    Ok(out)
}

#[test]
fn plane_sets_keep_the_index_next_to_the_locator() {
    // An indexed reader without checksums requires the records to run from
    // the primary's end to the index and the locator to follow the index:
    // the table must sit inside the primary.
    let (f, _, _, _) = plane_file(&primary(true)).unwrap();
    let n = f.len();
    let footer = &f[n - 37..];
    assert_eq!(&footer[..4], b"sLOC");
    let field = |r: core::ops::Range<usize>| u64::from_le_bytes(footer[r].try_into().unwrap());
    let (offset, index_len) = (field(13..21) as usize, field(21..29) as usize);
    assert_eq!(field(29..37) as usize, n);
    assert_eq!(offset + index_len, n - 37, "index next to the locator");
    assert_eq!(&f[offset..offset + 4], b"pIDX");
    let (_, end) = csum(&f);
    assert_eq!(&f[end..end + 4], b"DATA", "table before the primary's DATA");
    let primary_len = plane_reader(&f, PlaneSetLimits::default())
        .unwrap()
        .primary_len() as usize;
    assert!(end <= primary_len, "table inside the primary");
    let cs = chunks(&f);
    let records: Vec<_> = cs.iter().filter(|c| &c.1 == b"pDAT").collect();
    assert_eq!(records[0].0, primary_len, "records from the primary's end");
    for w in records.windows(2) {
        assert_eq!(w[1].0, w[0].2.end, "contiguous records");
    }
    assert_eq!(
        records.last().unwrap().2.end,
        offset,
        "records up to the index"
    );
}

#[test]
fn plane_sets_put_the_table_in_the_primary_and_verify_reads() {
    let (plain, id, a, pixels) = plane_file(&primary(false)).unwrap();
    let (_, primary_px) = decode(&primary(false)).unwrap();
    let (f, id2, _, _) = plane_file(&primary(true)).unwrap();
    assert_eq!(id, id2);
    // Plane directories hold absolute offsets, which the 3-byte header record
    // and the table shift; the chunk sequence is otherwise the plain file's.
    let tags = |f: &[u8]| -> Vec<[u8; 4]> {
        chunks(f)
            .into_iter()
            .map(|c| c.1)
            .filter(|t| t != b"cSUM")
            .collect()
    };
    assert_eq!(tags(&f), tags(&plain));
    // Without the table and the record, the header and every chunk but the
    // index and the locator (which hold the shifted offsets) are the plain
    // file's, byte for byte.
    let stripped = strip_all(&f);
    let (cf, cp) = (chunks(&stripped), chunks(&plain));
    assert_eq!(stripped[..cf[0].0], plain[..cp[0].0]);
    for (x, y) in cf.iter().zip(&cp) {
        if !matches!(&x.1, b"pIDX" | b"sLOC") {
            assert_eq!(stripped[x.2.clone()], plain[y.2.clone()], "{:?}", x.1);
        }
    }
    let (at, end) = csum(&f);
    assert_eq!(&f[end..end + 4], b"DATA", "table before the primary's DATA");
    let cs = chunks(&f);
    assert_eq!(cs[cs.len() - 2].1, *b"pIDX");
    let r = rebuilt(&f);
    let diff = r.iter().zip(&f).position(|(x, y)| x != y);
    assert_eq!(
        (r.len(), diff),
        (f.len(), None),
        "rebuilt plane file (at {at})"
    );
    assert!(at < cs.iter().find(|c| &c.1 == b"pDAT").unwrap().0);
    // The default verifies every piece a read overlaps; the partial-piece
    // policy is an explicit opt-in.
    let full = PlaneSetLimits::default();
    let partial = full.with_checksum_round_out(false);
    for limits in [partial, full] {
        assert_eq!(read_region(&f, id, limits).unwrap(), pixels);
        let mut r = plane_reader(&f, limits).unwrap();
        assert_eq!(r.read_attachment(a).unwrap().len(), 300_000);
    }
    assert_eq!(decode(&f).unwrap().1, primary_px);
    let mut out = Vec::new();
    decode_reader(f.as_slice(), &Limits::DEFAULT, |i, y, r| {
        rows_into(i, &mut out, y, r)
    })
    .unwrap();
    assert_eq!(out, primary_px);
    // A changed tile byte: whole-file decoders fail, and so does the
    // indexed reader by default; the partial-piece policy checks only the
    // pieces wholly inside what it fetches.
    let records: Vec<_> = cs.iter().filter(|c| &c.1 == b"pDAT").collect();
    let mut bad = f.clone();
    bad[records[0].2.start + 1] ^= 0x10;
    assert_eq!(decode(&bad).err(), Some(MISMATCH));
    assert_ne!(read_region(&bad, id, partial).err(), Some(MISMATCH));
    assert_eq!(read_region(&bad, id, full).err(), Some(MISMATCH));
    // Coverage: a tile read verifies every byte it used by default; the
    // partial-piece policy reports the bytes it left unverified.
    let coverage = |limits| {
        let mut r = plane_reader(&f, limits).unwrap();
        let mut out = vec![0u8; 128];
        r.decode_region(id, 0, Rect::new(0, 0, 8, 8), &mut out, 16)
            .unwrap();
        r.checksum_coverage()
    };
    let c = coverage(full);
    assert!(
        c.checking && c.verified_bytes > 0 && c.unverified_bytes == 0,
        "{c:?}"
    );
    let c = coverage(partial);
    assert!(c.checking && c.unverified_bytes > 0, "{c:?}");
    let c = coverage(full.with_checksum_verification(false));
    assert_eq!(c, Default::default());
    let off = PlaneSetLimits::default().with_checksum_verification(false);
    // Off: the tile's own decode meets the change (here its signature).
    assert_eq!(
        read_region(&bad, id, off.with_checksum_round_out(true)).err(),
        Some(Error::NotImij)
    );
    // A changed byte in the middle of the attachment is inside whole pieces.
    let blob = records.iter().max_by_key(|c| c.2.len()).unwrap().2.clone();
    let mut bad = f.clone();
    bad[blob.start + 150_000] ^= 1;
    let mut r = plane_reader(&bad, PlaneSetLimits::default()).unwrap();
    assert_eq!(r.read_attachment(a).err(), Some(MISMATCH));
    let mut r = plane_reader(&bad, off).unwrap();
    assert_eq!(r.read_attachment(a).unwrap().len(), 300_000);
    // A changed index byte fails at open.
    let (_, idx) = find(&f, b"pIDX").unwrap();
    let mut bad = f.clone();
    bad[idx.start + idx.len() / 2] ^= 1;
    assert!(plane_reader(&bad, full).is_err());
    // The builder verifies its primary and refuses a table it cannot redo.
    let mut bad_primary = primary(true);
    let (_, b) = csum(&bad_primary);
    bad_primary[b - 1] ^= 1;
    assert_eq!(plane_file(&bad_primary).err(), Some(MISMATCH));
    let mut unknown = strip(&primary(true));
    let rec = record_at(&unknown) + 1;
    unknown[rec] = 127;
    let data = find(&unknown, b"DATA").unwrap().0;
    unknown.splice(data..data, b"cSUM\x03\x7f\x00".iter().copied());
    assert_eq!(
        plane_file(&unknown).err(),
        Some(Error::Unsupported("unknown file checksum algorithm"))
    );
    // Without a table, the gap before the locator must be empty.
    let mut gap = plain.clone();
    let footer = gap.len() - 37;
    gap.splice(footer..footer, b"xxxx".iter().copied());
    assert!(plane_reader(&gap, PlaneSetLimits::default()).is_err());
}

/// The indexed plane reader walks the primary's chunks to find its
/// checksum table, under the chunk rules every decoder applies to them
/// (inventory B11: it skipped tags, criticality and repeats): a primary
/// chunk turned into a second eXIf, an invalid tag or an unknown critical
/// chunk fails at open, as a full decode of the file fails.
#[test]
fn plane_readers_apply_the_chunk_rules_to_the_primary() {
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default().with_effort(0.0);
        cfg = cfg.with_checksum(true);
        cfg
    };
    let (info, px) = image(6, 5, Channels::Gray);
    let p = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta())
        .unwrap();
    let (f, _, _, _) = plane_file(&p).unwrap();
    plane_reader(&f, PlaneSetLimits::default()).unwrap();
    let (at, _) = find(&f, b"vnDr").unwrap();
    for (tag, want) in [
        (b"eXIf", Error::Corrupt("repeated chunk")),
        (b"v1Dr", Error::Corrupt("invalid chunk tag")),
        (b"VnDr", Error::Unsupported("unknown critical chunk")),
    ] {
        let mut bad = f.clone();
        bad[at..at + 4].copy_from_slice(tag);
        assert_eq!(decode(&bad).err(), Some(want.clone()), "{tag:?}");
        assert_eq!(
            plane_reader(&bad, PlaneSetLimits::default()).err(),
            Some(want),
            "{tag:?}"
        );
    }
}

/// `f` with its `vnDr` chunk turned, in place, into an open (length 0)
/// critical chunk of an unknown tag.
fn with_open_critical(f: &[u8]) -> Vec<u8> {
    let (at, payload) = find(f, b"vnDr").unwrap();
    assert_eq!(payload.start, at + 5, "one-byte length");
    let mut v = f.to_vec();
    v[at..at + 4].copy_from_slice(b"ZZZZ");
    v[at + 4] = 0;
    v
}

/// SPEC 1.1: a decoder that does not know a critical chunk fails with
/// `Unsupported`, and only critical chunks may be open, so an open chunk of
/// an unknown critical tag is `Unsupported` on every route that walks the
/// chunks: the matrix's `ch/open-unknown-critical` (decoders, metadata
/// remux, plane-set builder), and the plane-set reader, which walks the
/// primary's chunks to its table.
#[test]
fn open_unknown_critical_chunks_stop_the_plane_set_reader() {
    let (info, px) = image(6, 5, Channels::Gray);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default().with_effort(0.0);
        cfg = cfg.with_checksum(true);
        cfg
    };
    let primary = cfg
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta())
        .unwrap();
    let (set, ..) = plane_file(&primary).unwrap();
    assert!(plane_reader(&set, PlaneSetLimits::default()).is_ok());
    assert_eq!(
        plane_reader(&with_open_critical(&set), PlaneSetLimits::default()).err(),
        Some(Error::Unsupported("unknown critical chunk"))
    );
}
