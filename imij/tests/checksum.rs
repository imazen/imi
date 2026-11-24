//! Piecewise `cSUM` file checksum (SPEC 1.4), decoders: every read path
//! verifies the pieces (or skips them when told to), multi-threaded and
//! streaming decoders included, and changed, truncated, extended or
//! malformed files are rejected wherever the table sits.

mod common;
use common::checksum::*;

// ---- multi-threaded and streaming decoders -----------------------------------

#[test]
fn threaded_decodes_check_every_piece() {
    // 192x160 RGB with 16-row stripes, without and with 64-column groups:
    // several units and, at k = 10, many pieces. A changed byte in any piece
    // is reported on every thread count, whichever worker decodes it.
    for groups in [Some(0), Some(64)] {
        let (info, px) = image(192, 160, Channels::Rgb);
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_checksum(true);
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(16);
            });
            cfg = cfg.with_column_groups(groups);
            cfg
        };
        let f = cfg
            .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta())
            .unwrap();
        let base = strip(&f);
        for place in [Place::BeforeData, Place::Last] {
            let v = with_table(&base, 10, 8, place);
            let (a, b) = csum(&v);
            let data = find(&v, b"DATA").unwrap().1;
            let opaque = find(&v, b"vnDr").unwrap().1.start + 2;
            let mut at = vec![opaque, data.start + 3, data.end - 1];
            // The middle byte of every piece, in covered offsets, mapped back.
            at.extend((0..base.len().div_ceil(1024)).map(|p| {
                let c = (p * 1024 + 512).min(base.len() - 1);
                if c < a { c } else { c + (b - a) }
            }));
            for threads in [1, 2, 4, 8] {
                assert_eq!(via_threads_n(&v, threads).unwrap(), px);
                for &at in &at {
                    let mut bad = v.clone();
                    bad[at] ^= 0x55;
                    assert_eq!(
                        via_threads_n(&bad, threads).err(),
                        Some(MISMATCH),
                        "groups {groups:?} {place:?} byte {at} threads {threads}"
                    );
                }
            }
        }
    }
}

/// A row-at-a-time decoder never returns a row from a piece it has not
/// checked: with the table before DATA, a changed byte stops the decode
/// before the first row whose stripe needs that byte.
#[test]
fn streamed_decodes_stop_before_a_bad_piece() {
    let f = lossless_sized(192, 160, true);
    let (info, want) = decode(&f).unwrap();
    let rb = info.row_bytes();
    let v = with_table(&strip(&f), 10, 8, Place::BeforeData);
    let data = find(&v, b"DATA").unwrap().1;
    // A byte in the middle of the payload: later stripes are not decoded.
    let at = data.start + data.len() / 2;
    let mut bad = v.clone();
    bad[at] ^= 0x55;
    // The Read decoder reads each piece ahead and checks it before decoding.
    let mut rows = Vec::new();
    let r = decode_reader(
        Chunked {
            data: &bad,
            calls: 0,
        },
        &Limits::DEFAULT,
        |i, y, row| rows_into(i, &mut rows, y, row),
    );
    assert_eq!(r.err(), Some(MISMATCH));
    assert!(rows.len() < want.len(), "rows past the bad piece");
    assert_eq!(&rows[..], &want[..rows.len()]);
    // StreamDecoder: rows up to the stripe that needs the bad piece.
    let mut s = StreamDecoder::new(&bad).unwrap();
    let mut row = vec![0u8; rb];
    let mut got = Vec::new();
    let err = loop {
        match s.next_row(&mut row) {
            Ok(true) => got.extend_from_slice(&row),
            Ok(false) => panic!("decoded past a bad piece"),
            Err(e) => break e,
        }
    };
    assert_eq!(err, MISMATCH);
    assert!(got.len() < want.len());
    assert_eq!(&got[..], &want[..got.len()]);
    // A table at the end is compared after the rows.
    let last = with_table(&strip(&f), 10, 8, Place::Last);
    let data = find(&last, b"DATA").unwrap().1;
    let mut bad = last.clone();
    bad[data.end - 1] ^= 1;
    assert_eq!(via_reader(bad.as_slice()).err(), Some(MISMATCH));
    // With the table first, a changed header byte fails before any row.
    let mut bad = v.clone();
    bad[9] ^= 0x40;
    let mut rows = 0;
    let r = decode_reader(bad.as_slice(), &Limits::DEFAULT, |_, _, _| {
        rows += 1;
        Ok(())
    });
    assert!(r.is_err());
    assert_eq!(rows, 0);
}

// ---- rejection --------------------------------------------------------------------

#[test]
fn every_single_byte_change_is_rejected() {
    for (name, _, f) in files() {
        let base = strip(&f);
        for (place, v) in [
            ("before DATA", f.clone()),
            ("last", with_table(&base, f[record_at(&f)], 8, Place::Last)),
        ] {
            // The record names the algorithm, so a changed algorithm byte in
            // the table or the record is a mismatch (SPEC 1.4).
            for i in 0..v.len() {
                for mask in [1u8, 2, 4, 8, 16, 32, 64, 128, 0xFF] {
                    let mut bad = v.clone();
                    bad[i] ^= mask;
                    rejected(&bad, None, &format!("{name} {place}: byte {i} ^ {mask:#x}"));
                }
            }
        }
    }
}

#[test]
fn each_piece_detects_its_own_bytes() {
    let f = lossless_sized(64, 64, true);
    let base = strip(&f);
    for width in [4u8, 8, 16] {
        let v = with_table(&base, 10, width, Place::BeforeData);
        let (a, b) = csum(&v);
        let pieces = base.len().div_ceil(1024);
        assert!(pieces >= 4);
        for p in 0..pieces {
            // The middle byte of piece p (in DATA past piece 0's header and
            // metadata), in covered offsets, mapped back to the file.
            let c = (p * 1024 + 512).min(base.len() - 1);
            let at = if c < a { c } else { c + (b - a) };
            let mut bad = v.clone();
            bad[at] ^= 0x20;
            assert_eq!(decode(&bad).err(), Some(MISMATCH), "piece {p}");
            // Only piece p's digest disagrees: replacing it repairs the file.
            let mut cov = strip(&bad);
            let rec = record_at(&cov);
            cov[rec] = 10;
            let d = xxhash_rust::xxh3::xxh3_128(&cov[p * 1024..((p + 1) * 1024).min(cov.len())]);
            let first = b - pieces * width as usize;
            bad[first + p * width as usize..][..width as usize]
                .copy_from_slice(&d.to_le_bytes()[..width as usize]);
            // (`probe` checks the container and the table, not the pixels.)
            assert!(probe(&bad).is_ok(), "piece {p} repaired");
        }
    }
}

#[test]
fn large_piece_sizes_on_every_path() {
    // k up to 40 is valid: one piece covers these files whatever k is, and
    // pieces of 2^32 bytes or more must not trip 32-bit offset arithmetic.
    for (name, plain, f) in files() {
        let (_, px) = decode(&plain).unwrap();
        let base = strip(&f);
        for k in [20, 31, 32, 33, 40] {
            for place in [Place::BeforeData, Place::Last] {
                let what = format!("{name} k={k} {place:?}");
                let v = with_table(&base, k, 8, place);
                accepted(&v, &px, &what);
                accepted_on(&OFF_PATHS, &v, &px, &format!("{what}, off"));
                let opaque = find(&v, b"vnDr").unwrap().1.start + 2;
                let mut bad = v.clone();
                bad[opaque] ^= 0x20;
                rejected(&bad, Some(&MISMATCH), &what);
            }
        }
    }
}

#[test]
fn changes_the_structure_cannot_see_fail_as_a_checksum_mismatch() {
    for (name, plain, f) in files() {
        let (_, px) = decode(&plain).unwrap();
        for (what, v) in variants(&f) {
            let cs = chunks(&v);
            for tag in [b"vnDr", b"iCCP"] {
                let r = cs.iter().find(|c| &c.1 == tag).unwrap().2.clone();
                let mut bad = v.clone();
                bad[r.start + 2] ^= 0x20;
                rejected(&bad, Some(&MISMATCH), &format!("{name} {what}"));
                // Verification off decodes it as a decoder without checksums.
                accepted_on(&OFF_PATHS, &bad, &px, &format!("{name} {what}, off"));
            }
        }
    }
}

/// `f` with its cSUM payload rebuilt by `edit`, at the same place.
fn edit_table(f: &[u8], edit: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let (a, _) = csum(f);
    let (_, r) = find(f, b"cSUM").unwrap();
    let mut payload = f[r.clone()].to_vec();
    edit(&mut payload);
    let mut c = b"cSUM".to_vec();
    put_varint(&mut c, payload.len() as u64 + 1);
    c.extend_from_slice(&payload);
    [&f[..a], &c[..], &f[r.end..]].concat()
}

#[test]
fn malformed_tables_and_records() {
    for (name, plain, f) in files() {
        let (_, px) = decode(&plain).unwrap();
        let base = strip(&f);
        let v = with_table(&base, 10, 8, Place::BeforeData);
        let covered = base.len() as u64;
        for (what, bad, why) in [
            (
                "covered_len + 1",
                edit_table(&v, |p| {
                    let mut q = vec![0, 8, 10];
                    put_varint(&mut q, covered + 1);
                    q.extend_from_slice(&p[3 + varint_len(covered)..]);
                    *p = q;
                }),
                "cSUM length does not match the file",
            ),
            (
                "one digest short",
                edit_table(&v, |p| p.truncate(p.len() - 8)),
                "cSUM digest count",
            ),
            ("width 5", edit_table(&v, |p| p[1] = 5), "cSUM digest width"),
            ("k = 9", edit_table(&v, |p| p[2] = 9), "cSUM piece size"),
            ("k = 41", edit_table(&v, |p| p[2] = 41), "cSUM piece size"),
            ("empty", edit_table(&v, |p| p.clear()), "empty cSUM chunk"),
        ] {
            let bad = if what == "covered_len + 1" {
                // One more covered byte needs one more digest when it starts a
                // piece; keep the digest count right so the length is what
                // disagrees.
                let pieces = (covered + 1).div_ceil(1024) as usize;
                edit_table(&bad, |p| {
                    let head = 3 + varint_len(covered + 1);
                    p.resize(head + pieces * 8, 0);
                })
            } else {
                bad
            };
            rejected(&bad, None, &format!("{name}: {what} ({why})"));
            accepted_on(&OFF_PATHS, &bad, &px, &format!("{name}: {what}, off"));
        }
        // A table whose piece size differs from the header record.
        let mut bad = with_table(&base, 11, 8, Place::BeforeData);
        let rec = record_at(&bad);
        bad[rec] = 10;
        rejected(&bad, None, &format!("{name}: k differs from the record"));
        // A byte appended to the file.
        let mut longer = v.clone();
        longer.push(0);
        rejected(&longer, None, &format!("{name}: a byte appended"));
        // A second table.
        let (a, b) = csum(&v);
        let twice = [&v[..b], &v[a..b], &v[b..]].concat();
        rejected(&twice, None, &format!("{name}: two tables"));
        // A table without its header record, and a record without a table.
        let mut no_record = drop_record(&base);
        let data = find(&no_record, b"DATA").unwrap().0;
        no_record.splice(data..data, v[a..b].iter().copied());
        rejected(
            &no_record,
            Some(&Error::Corrupt("file checksum without its header record")),
            name,
        );
        accepted_on(
            &OFF_PATHS,
            &no_record,
            &px,
            &format!("{name}: no record, off"),
        );
        rejected(&base, Some(&Error::Corrupt("missing file checksum")), name);
        accepted_on(&OFF_PATHS, &base, &px, &format!("{name}: no table, off"));
        // A malformed record.
        let mut bad = v.clone();
        let rec = record_at(&bad);
        bad[rec] = 9;
        rejected(&bad, None, &format!("{name}: record k = 9"));
        accepted_on(&OFF_PATHS, &bad, &px, &format!("{name}: record k = 9, off"));
    }
}

#[test]
fn truncation_at_every_offset_is_rejected() {
    for (name, _, f) in files() {
        let base = strip(&f);
        for (place, v) in [
            ("before DATA", f.clone()),
            ("last", with_table(&base, f[record_at(&f)], 8, Place::Last)),
        ] {
            for cut in 0..v.len() {
                rejected(&v[..cut], None, &format!("{name} {place}: cut at {cut}"));
            }
        }
        // A stripped trailing table is detected through the header record.
        let last = with_table(&base, 16, 8, Place::Last);
        let (a, _) = csum(&last);
        rejected(
            &last[..a],
            Some(&Error::Corrupt("missing file checksum")),
            name,
        );
    }
}

#[test]
fn unknown_algorithms_are_skipped() {
    // ID 2 is BLAKE3-256 when that feature is compiled in.
    assert!(imij::HashAlgorithm::from_wire(127).is_none());
    assert!(imij::HashAlgorithm::from_wire(255).is_none());
    for (_, plain, f) in files() {
        let (_, px) = decode(&plain).unwrap();
        for payload in [vec![127], vec![127, 1, 2, 3], vec![255; 300]] {
            // The header record names the same unknown algorithm.
            let mut base = strip(&f);
            let rec = record_at(&base) + 1;
            base[rec] = payload[0];
            let mut c = b"cSUM".to_vec();
            put_varint(&mut c, payload.len() as u64 + 1);
            c.extend_from_slice(&payload);
            for at in [find(&base, b"DATA").unwrap().0, base.len()] {
                let v = [&base[..at], &c[..], &base[at..]].concat();
                accepted(&v, &px, "unknown algorithm");
                accepted_on(&OFF_PATHS, &v, &px, "unknown algorithm, off");
                assert!(
                    probe(&v)
                        .unwrap()
                        .1
                        .chunks
                        .iter()
                        .all(|c| &c.tag != b"cSUM")
                );
            }
        }
    }
}

/// An open DATA chunk (no length) is covered like any other bytes; the table
/// may precede or follow it.
#[test]
fn open_data_is_covered() {
    for (name, plain, f) in files() {
        let (_, px) = decode(&plain).unwrap();
        let base = strip(&f);
        let (at, r) = find(&base, b"DATA").unwrap();
        let mut open = base[..at + 4].to_vec();
        open.push(0);
        open.extend_from_slice(&base[r.start..]);
        for place in [Place::BeforeData, Place::Last] {
            let v = with_table(&open, 10, 8, place);
            accepted(&v, &px, &format!("{name} open DATA {place:?}"));
            let (dat, _) = find(&v, b"DATA").unwrap();
            let mut bad = v.clone();
            bad[dat + 10] ^= 1;
            rejected(&bad, None, &format!("{name} open DATA {place:?}, changed"));
        }
    }
}

/// What a decoder without `cSUM` support sees: an unknown ancillary chunk
/// and an ignorable header record. Verification off reads files that way;
/// the same files were decoded by the `main` build (see the PR).
#[test]
fn older_decoders_skip_it_as_unknown_ancillary() {
    for (name, plain, f) in files() {
        let (_, px) = decode(&plain).unwrap();
        for (what, v) in variants(&f) {
            let (a, _) = csum(&v);
            let mut old = v.clone();
            old[a..a + 4].copy_from_slice(b"xSUM");
            accepted_on(&OFF_PATHS, &old, &px, &format!("{name} {what}"));
            accepted_on(&OFF_PATHS, &v, &px, &format!("{name} {what}"));
            // With verification on, the header record wants its table.
            assert_eq!(
                probe(&old).err(),
                Some(Error::Corrupt("missing file checksum"))
            );
            let kept = decode_reader(old.as_slice(), &OFF, |_, _, _| Ok(()))
                .unwrap()
                .1
                .chunks;
            assert!(kept.iter().any(|c| &c.tag == b"xSUM"), "{name} {what}");
        }
    }
}

/// Rows a row-at-a-time route returned before it stopped, and its result.
fn rows_until_error(route: &str, f: &[u8]) -> (Vec<u8>, Result<(), Error>) {
    let mut rows = Vec::new();
    let r = match route {
        // Parsing reads the payload head and record lengths before any
        // piece is checked; a mismatch explains a parse error there, so it
        // is the error reported (mainfix W1).
        "StreamDecoder" => StreamDecoder::new(f).and_then(|s| stream_rows(s, &mut rows)),
        "decode_reader" => decode_reader(
            Chunked { data: f, calls: 0 },
            &Limits::DEFAULT,
            |i, y, row| rows_into(i, &mut rows, y, row),
        )
        .map(drop),
        "DecodeSession::decode_rows" => out_len(f).and_then(|(info, _)| {
            let mut row = vec![0u8; info.row_bytes()];
            session(Limits::DEFAULT)
                .decode_rows(f, &mut row, |i, y, r| rows_into(i, &mut rows, y, r))
                .map(drop)
        }),
        "DecodeSession::decode_reader" => session(Limits::DEFAULT)
            .decode_reader(Chunked { data: f, calls: 0 }, |i, y, r| {
                rows_into(i, &mut rows, y, r)
            })
            .map(drop),
        _ => unreachable!(),
    };
    (rows, r)
}

/// Lossy files with exact blocks: each unit's exact record follows its
/// lossy record (and the alpha record precedes it), so a stripe's last
/// record in file order is its last group's exact record. A changed byte
/// anywhere in the payload stops every row route with the checksum error
/// before it returns a row that needs the byte.
#[test]
fn row_routes_check_exact_records_before_their_stripe() {
    for ch in [Channels::Rgb, Channels::Rgba] {
        let (w, h) = (128u32, 192u32);
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        let px: Vec<u8> = (0..info.row_bytes() * h as usize)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect();
        let mut cfg = imij::ImiEncoderConfig::lossy(80.0)
            .with_decode_speed(imij::DecodeSpeed::Compact)
            .with_checksum(true);
        cfg = cfg.tune_lossy(|t| {
            t.exact_blocks = Some(1000.0);
        });
        let f = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let (_, want) = decode(&f).unwrap();
        // Nearly every block is exact (noise: the color channels come back
        // as the source).
        let same = want
            .chunks(ch.count())
            .zip(px.chunks(ch.count()))
            .filter(|(a, b)| a[..3] == b[..3])
            .count();
        assert!(same * 10 > (w * h) as usize * 9, "{ch:?}: {same} exact");
        let v = with_table(&strip(&f), 10, 8, Place::BeforeData);
        assert_eq!(decode(&v).unwrap().1, want);
        let data = find(&v, b"DATA").unwrap().1;
        for route in [
            "StreamDecoder",
            "decode_reader",
            "DecodeSession::decode_rows",
            "DecodeSession::decode_reader",
        ] {
            let (rows, r) = rows_until_error(route, &v);
            assert_eq!(r, Ok(()), "{route} {ch:?}");
            assert_eq!(rows, want, "{route} {ch:?}");
            for at in data.clone().step_by(61) {
                let mut bad = v.clone();
                bad[at] ^= 0x55;
                let (rows, r) = rows_until_error(route, &bad);
                assert_eq!(r, Err(MISMATCH), "{route} {ch:?} byte {at}");
                assert!(rows.len() < want.len(), "{route} {ch:?} byte {at}");
                assert!(
                    rows == want[..rows.len()],
                    "{route} {ch:?} byte {at}: a row from an unchecked piece \
                     ({} rows returned, first wrong row {:?})",
                    rows.len() / info.row_bytes(),
                    rows.chunks(info.row_bytes())
                        .zip(want.chunks(info.row_bytes()))
                        .position(|(a, b)| a != b)
                );
            }
        }
    }
}

/// SPEC 0: a decoder without a profile rejects a file that needs it from the
/// header. A build without `extended` rejects 32- and 64-bit files with
/// `Unsupported` on every decoding route before it walks the chunks, so a
/// damaged checksum piece does not turn that into a mismatch.
#[cfg(not(feature = "extended"))]
#[test]
fn builds_without_extended_reject_wide_files_before_the_checksum() {
    for (sample, want) in [
        (SampleType::U32, "32-bit samples (extended tier)"),
        (SampleType::I32, "32-bit samples (extended tier)"),
        (SampleType::I8, "signed samples (extended tier)"),
        (SampleType::I16, "signed samples (extended tier)"),
        (SampleType::F64, "64-bit samples (extended tier)"),
    ] {
        let info = ImageInfo::new(24, 16, Channels::Gray, sample);
        let px: Vec<u8> = (0..info.row_bytes() * 16).map(|i| (i % 7) as u8).collect();
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_checksum(true);
            cfg
        };
        let f = cfg
            .encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let data = find(&f, b"DATA").unwrap().1;
        let mut bad = f.clone();
        bad[data.end - 1] ^= 0x55;
        for file in [&f, &bad] {
            for (name, path) in PATHS {
                let r = path(file);
                if name == "probe" {
                    // Inspection reads the whole file and reports it.
                    let want = if file == &f { None } else { Some(MISMATCH) };
                    assert_eq!(r.err(), want, "{sample:?}");
                    continue;
                }
                assert_eq!(r.err(), Some(Error::Unsupported(want)), "{name} {sample:?}");
            }
        }
    }
}
