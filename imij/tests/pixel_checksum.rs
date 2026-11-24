//! Optional pSUM: actual-output verification on complete and partial routes.
mod common;
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::*;

const MISMATCH: Error = Error::Corrupt("pixel checksum mismatch");
fn image(w: u32, h: u32, ch: Channels) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, ch, SampleType::U8);
    let pixels = (0..info.checked_buffer_len().unwrap())
        .map(|i| ((i * 37) ^ (i >> 6)) as u8)
        .collect();
    (info, pixels)
}
fn plain(info: &ImageInfo, pixels: &[u8], csum: bool) -> Vec<u8> {
    {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_checksum(csum);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg = cfg.with_column_groups(Some(32));
        cfg
    }
    .encode_samples(
        imij::ImageRef::new(pixels, info.row_bytes(), info),
        &Metadata::default(),
    )
    .unwrap()
}
fn payload(file: &[u8]) -> std::ops::Range<usize> {
    let (_, at, end) = common::chunks(file)
        .into_iter()
        .find(|c| c.0 == *b"pSUM")
        .unwrap();
    at + 4 + common::varint(&file[at + 4..]).1..end
}
fn with_payload(file: &[u8], body: &[u8]) -> Vec<u8> {
    let (_, at, end) = common::chunks(file)
        .into_iter()
        .find(|c| c.0 == *b"pSUM")
        .unwrap();
    let mut out = file[..at].to_vec();
    out.extend_from_slice(b"pSUM");
    common::put_varint(&mut out, body.len() as u64 + 1);
    out.extend_from_slice(body);
    out.extend_from_slice(&file[end..]);
    out
}
fn check_routes(file: &[u8], info: &ImageInfo, expected: Option<&[u8]>) {
    let result = decode(file);
    match expected {
        Some(want) => assert_eq!(result.unwrap().1, want),
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let rb = info.row_bytes();
    let stride = rb + 11;
    for threads in [1, 2, 4, 8] {
        let mut pixels = vec![0xa5; stride * info.height as usize];
        let result = decode_into_threads(file, &mut pixels, stride, threads);
        match expected {
            Some(want) => {
                result.unwrap();
                for (row, src) in pixels.chunks(stride).zip(want.chunks(rb)) {
                    assert_eq!(&row[..rb], src);
                    assert!(row[rb..].iter().all(|&x| x == 0xa5));
                }
            }
            None => assert_eq!(result.err(), Some(MISMATCH)),
        }
    }
    let mut output = vec![0; rb * info.height as usize];
    let result = decode_into(file, &mut output, rb);
    match expected {
        Some(want) => {
            result.unwrap();
            assert_eq!(output, want);
        }
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let mut session = DecodeSession::new(DecodePolicy::default());
    let result = session.decode(file);
    match expected {
        Some(want) => assert_eq!(result.unwrap().1, want),
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let result = session.decode_into(file, &mut output, rb);
    match expected {
        Some(want) => {
            result.unwrap();
            assert_eq!(output, want);
        }
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let mut row = vec![0; rb];
    let mut got = Vec::new();
    let result = session.decode_rows(file, &mut row, |_, _, row| {
        got.extend_from_slice(row);
        Ok(())
    });
    match expected {
        Some(want) => {
            result.unwrap();
            assert_eq!(got, want);
        }
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let mut got = Vec::new();
    let result = decode_reader(file, &Limits::DEFAULT, |_, _, row| {
        got.extend_from_slice(row);
        Ok(())
    });
    match expected {
        Some(want) => {
            result.unwrap();
            assert_eq!(got, want);
        }
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let result = session.decode_reader(file, |_, _, _| Ok(()));
    match expected {
        Some(_) => {
            result.unwrap();
        }
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
    let mut frames = FrameDecoder::new(file).unwrap();
    frames.next_frame().unwrap();
    let result = frames.decode_into(&mut output, rb, 4);
    match expected {
        Some(_) => {
            result.unwrap();
            assert_eq!(
                frames.pixel_checksum_status(),
                PixelChecksumStatus::Verified
            );
        }
        None => assert_eq!(result.err(), Some(MISMATCH)),
    }
}

#[test]
fn actual_outputs_all_routes_and_final_row_failure_is_sticky() {
    let (info, pixels) = image(67, 65, Channels::Rgba);
    let file = add_pixel_checksum(plain(&info, &pixels, false)).unwrap();
    check_routes(&file, &info, Some(&pixels));
    let mut bad = file.clone();
    let range = payload(&bad);
    bad[range.end - 1] ^= 1;
    check_routes(&bad, &info, None);
    let mut stream = StreamDecoder::new(&bad).unwrap();
    let mut row = vec![0; info.row_bytes()];
    assert_eq!(stream.pixel_checksum_status(), PixelChecksumStatus::Pending);
    for _ in 0..info.height - 1 {
        assert!(stream.next_row(&mut row).unwrap());
    }
    assert_eq!(stream.pixel_checksum_status(), PixelChecksumStatus::Pending);
    assert_eq!(stream.next_row(&mut row), Err(MISMATCH));
    assert_eq!(stream.next_row(&mut row), Err(MISMATCH));
    assert_eq!(stream.pixel_checksum_status(), PixelChecksumStatus::Pending);
    let mut stream = StreamDecoder::new(&file).unwrap();
    for _ in 0..info.height {
        assert!(stream.next_row(&mut row).unwrap());
    }
    assert_eq!(
        stream.pixel_checksum_status(),
        PixelChecksumStatus::Verified
    );
    assert!(!stream.next_row(&mut row).unwrap());
}

#[test]
fn roi_requires_whole_primary_pixels_for_verification() {
    let (info, pixels) = image(67, 65, Channels::Rgb);
    let file = add_pixel_checksum(plain(&info, &pixels, false)).unwrap();
    let mut reader = RegionReader::open(SliceReader::new(&file)).unwrap();
    let mut crop = vec![0; 3 * 3 * 5];
    reader
        .decode_region(Rect::new(1, 2, 3, 5), &mut crop, 9)
        .unwrap();
    assert_eq!(reader.pixel_checksum_status(), PixelChecksumStatus::Pending);
    let mut full = vec![0; pixels.len()];
    reader
        .decode_region(Rect::new(0, 0, 67, 65), &mut full, info.row_bytes())
        .unwrap();
    assert_eq!(full, pixels);
    assert_eq!(
        reader.pixel_checksum_status(),
        PixelChecksumStatus::Verified
    );
    let mut bad = file.clone();
    let range = payload(&bad);
    bad[range.end - 1] ^= 1;
    let mut reader = RegionReader::open(SliceReader::new(&bad)).unwrap();
    reader
        .decode_region(Rect::new(1, 2, 3, 5), &mut crop, 9)
        .unwrap();
    assert_eq!(reader.pixel_checksum_status(), PixelChecksumStatus::Pending);
    assert_eq!(
        reader.decode_region(Rect::new(0, 0, 67, 65), &mut full, info.row_bytes()),
        Err(MISMATCH)
    );
}

#[test]
fn lossy_and_near_lossless_hash_reconstruction() {
    let (info, pixels) = image(33, 17, Channels::Rgba);
    let lossy = imij::ImiEncoderConfig::lossy(20.0)
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let near = imij::ImiEncoderConfig::default()
        .tune_lossless(|t| t.max_error = [11; 4])
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    for original in [lossy, near] {
        let expected = decode(&original).unwrap().1;
        assert_ne!(
            expected, pixels,
            "fixture must distinguish reconstruction from source"
        );
        let file = add_pixel_checksum(original).unwrap();
        check_routes(&file, &info, Some(&expected));
    }
}

#[test]
fn canvas_table_checks_cropped_updates_and_empty_frames() {
    let (info, first) = image(64, 48, Channels::Rgba);
    let mut changed = first.clone();
    let rb = info.row_bytes();
    for y in 15..19 {
        for x in 25..29 {
            changed[y * rb + x * 4] ^= 0x7f;
        }
    }
    let mut enc = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    for pixels in [&first, &changed, &changed] {
        enc.push_frame(pixels, rb, FrameDuration::from_millis(1))
            .unwrap();
    }
    let original = enc.finish().unwrap();
    let file = add_pixel_checksum(original).unwrap();
    let mut decoder = FrameDecoder::new(&file).unwrap();
    let mut canvas = vec![0; first.len()];
    decoder.next_frame().unwrap();
    decoder.decode_into(&mut canvas, rb, 2).unwrap();
    assert_eq!(canvas, first);
    let crop = decoder.next_frame().unwrap().unwrap();
    assert!(crop.width < info.width);
    assert!(crop.uses_previous);
    canvas[0] ^= 1; // Wrong reference OUTSIDE the update, file bytes unchanged.
    assert_eq!(decoder.decode_into(&mut canvas, rb, 4), Err(MISMATCH));
    // A failed in-place inter decode may have written the update already.
    // Restore the correct previous reference before retrying that frame.
    canvas.copy_from_slice(&first);
    decoder.decode_into(&mut canvas, rb, 4).unwrap();
    assert_eq!(canvas, changed);
    let empty = decoder.next_frame().unwrap().unwrap();
    assert_eq!((empty.width, empty.height), (0, 0));
    canvas[0] ^= 1;
    assert_eq!(decoder.decode_into(&mut canvas, rb, 4), Err(MISMATCH));
    canvas[0] ^= 1;
    decoder.decode_into(&mut canvas, rb, 4).unwrap();
    assert_eq!(canvas, changed);
    assert_eq!(
        decoder.pixel_checksum_status(),
        PixelChecksumStatus::Verified
    );
}

#[test]
fn tables_validate_known_syntax_and_unknown_versions_remain_unverified() {
    let (info, pixels) = image(17, 3, Channels::Gray);
    let file = add_pixel_checksum(plain(&info, &pixels, false)).unwrap();
    for body in [
        vec![],
        vec![1],
        vec![1, 0],
        vec![1, 0, 4, 0, 1],
        vec![1, 0, 8, 1, 1],
        vec![1, 0, 8, 0, 0],
        vec![1, 0, 8, 0, 1],
        vec![
            1, 0, 8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ] {
        let bad = with_payload(&file, &body);
        assert!(matches!(decode(&bad), Err(Error::Corrupt(_))), "{body:?}");
        assert!(
            matches!(
                decode_reader(bad.as_slice(), &Limits::DEFAULT, |_, _, _| Ok(())),
                Err(Error::Corrupt(_))
            ),
            "reader {body:?}"
        );
        assert!(
            matches!(
                RegionReader::open(SliceReader::new(&bad)),
                Err(Error::Corrupt(_))
            ),
            "region {body:?}"
        );
    }
    for body in [vec![2], vec![1, 127]] {
        let unknown = with_payload(&file, &body);
        assert_eq!(decode(&unknown).unwrap().1, pixels);
        let mut stream = StreamDecoder::new(&unknown).unwrap();
        let mut row = vec![0; info.row_bytes()];
        while stream.next_row(&mut row).unwrap() {}
        assert_eq!(
            stream.pixel_checksum_status(),
            PixelChecksumStatus::Unsupported
        );
        assert!(matches!(
            add_pixel_checksum(unknown),
            Err(Error::InvalidInput(_))
        ));
    }
    let (_, at, end) = common::chunks(&file)
        .into_iter()
        .find(|c| c.0 == *b"pSUM")
        .unwrap();
    let mut duplicate = file.clone();
    duplicate.splice(at..at, file[at..end].iter().copied());
    assert!(matches!(decode(&duplicate), Err(Error::Corrupt(_))));
    let mut after = file.clone();
    let chunk: Vec<_> = after.drain(at..end).collect();
    after.extend(chunk);
    assert_eq!(
        decode(&after).err(),
        Some(Error::Corrupt("pSUM after DATA"))
    );
}

#[test]
fn metadata_only_remux_preserves_coverage_and_encoded_checksum_is_independent() {
    let (info, pixels) = image(19, 5, Channels::Rgb);
    let meta = Metadata {
        xmp: Some(b"metadata ".repeat(512)),
        ..Default::default()
    };
    let original = imij::ImiEncoderConfig::default()
        .with_checksum(true)
        .encode_samples(imij::ImageRef::new(&pixels, info.row_bytes(), &info), &meta)
        .unwrap();
    let file = add_pixel_checksum(original).unwrap();
    let packed = compress_metadata(&file, 8192).unwrap();
    assert!(packed.len() < file.len());
    assert_eq!(&file[payload(&file)], &packed[payload(&packed)]);
    assert_eq!(decode(&packed).unwrap().1, pixels);
    let (_, metadata) = probe(&packed).unwrap();
    assert!(metadata.chunks.iter().all(|c| c.tag != *b"pSUM"));
    let reencoded = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &metadata,
        )
        .unwrap();
    assert_eq!(
        StreamDecoder::new(&reencoded)
            .unwrap()
            .pixel_checksum_status(),
        PixelChecksumStatus::Absent
    );
    let mut bad = packed.clone();
    let range = payload(&bad);
    bad[range.end - 1] ^= 1;
    assert_eq!(
        decode(&bad).err(),
        Some(Error::Corrupt("file checksum mismatch"))
    );
    assert_eq!(
        decode_with_limits(&bad, &Limits::DEFAULT.with_checksum_verification(false)).err(),
        Some(MISMATCH)
    );
    let mut meta = Metadata::default();
    meta.chunks.push(Chunk {
        tag: *b"pSUM",
        data: vec![2],
    });
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .encode_samples(imij::ImageRef::new(&pixels, info.row_bytes(), &info), &meta),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn uniform_and_mixed_pages_use_each_page_descriptor_and_ordinal() {
    let cfg = FrameConfig::new(imij::ImiEncoderConfig::default());
    let mut uniform = FrameEncoder::pages(
        ImageInfo::new(17, 3, Channels::Gray, SampleType::U8),
        Metadata::default(),
        cfg.clone(),
    );
    let mut mixed = FrameEncoder::mixed_pages(cfg);
    let mut uniform_sources = Vec::new();
    let mut mixed_sources = Vec::new();
    for i in 0..3 {
        let (info, source) = image(17 + i * 3, 3 + i, Channels::Gray);
        uniform.push_page(&source, info.row_bytes(), &info).unwrap();
        uniform_sources.push((info, source));
        let (info, source) = image(
            19 + i * 2,
            5 + i,
            if i == 0 {
                Channels::Gray
            } else {
                Channels::Rgba
            },
        );
        mixed
            .push_page_with(
                &source,
                info.row_bytes(),
                &info,
                &Metadata::default(),
                &imij::ImiEncoderConfig::default(),
            )
            .unwrap();
        mixed_sources.push((info, source));
    }
    for (file, sources) in [
        (uniform.finish().unwrap(), uniform_sources),
        (mixed.finish().unwrap(), mixed_sources),
    ] {
        let file = add_pixel_checksum(file).unwrap();
        let mut decoder = FrameDecoder::new(&file).unwrap();
        for (ordinal, (info, source)) in sources.iter().enumerate() {
            let fh = decoder.next_frame().unwrap().unwrap();
            assert_eq!(fh.index, ordinal);
            let stride = info.row_bytes() + 9;
            let mut out = vec![0xa5; stride * info.height as usize];
            decoder.decode_into(&mut out, stride, 4).unwrap();
            for (row, source) in out.chunks(stride).zip(source.chunks(info.row_bytes())) {
                assert_eq!(&row[..info.row_bytes()], source);
                assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xa5));
            }
            assert_eq!(
                decoder.pixel_checksum_status(),
                PixelChecksumStatus::Verified
            );
        }
        let mut bad = file.clone();
        let at = payload(&bad).end - 1;
        bad[at] ^= 1;
        let mut decoder = FrameDecoder::new(&bad).unwrap();
        for (ordinal, (info, _)) in sources.iter().enumerate() {
            decoder.next_frame().unwrap();
            let mut out = vec![0; info.checked_buffer_len().unwrap()];
            let result = decoder.decode_into(&mut out, info.row_bytes(), 2);
            if ordinal == sources.len() - 1 {
                assert_eq!(result, Err(MISMATCH));
            } else {
                result.unwrap();
            }
        }
    }
}

#[test]
fn nested_preview_and_gain_map_checksums_attest_their_own_images() {
    let (info, source) = image(17, 5, Channels::Rgb);
    let (small, pixels) = image(3, 2, Channels::Gray);
    let nested = add_pixel_checksum(plain(&small, &pixels, false)).unwrap();
    let mut params = GainMapParams::default();
    for channel in &mut params.channels {
        channel.max = 2.0;
    }
    params.alternate_hdr_headroom = 2.0;
    let metadata = Metadata {
        gain_map: Some(GainMap {
            params,
            alt_cicp: None,
            alt_icc: None,
            image: nested.clone(),
        }),
        ..Default::default()
    };
    let mut outer = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&source, info.row_bytes(), &info),
            &metadata,
        )
        .unwrap();
    // pRVW is repeatable; the test supplies a complete independently sealed file.
    let at = common::chunks(&outer)
        .into_iter()
        .find(|c| c.0 == *b"DATA")
        .unwrap()
        .1;
    let mut chunk = b"pRVW".to_vec();
    common::put_varint(&mut chunk, nested.len() as u64 + 1);
    chunk.extend_from_slice(&nested);
    outer.splice(at..at, chunk);
    let outer = add_pixel_checksum(outer).unwrap();
    assert_eq!(decode(&outer).unwrap().1, source);
    let previews = previews(&outer).unwrap();
    assert_eq!(previews.len(), 1);
    assert_eq!(decode(previews[0]).unwrap().1, pixels);
    let gain = probe(&outer).unwrap().1.gain_map.unwrap();
    assert_eq!(decode(&gain.image).unwrap().1, pixels);
    let mut bad = nested.clone();
    let at = payload(&bad).end - 1;
    bad[at] ^= 1;
    assert_eq!(decode(&bad).err(), Some(MISMATCH));
}

#[test]
fn unknown_encoded_checksum_algorithm_cannot_be_retained_by_finalizer() {
    let (info, pixels) = image(3, 2, Channels::Rgb);
    let mut file = plain(&info, &pixels, true);
    let (_, at, _) = common::chunks(&file)
        .into_iter()
        .find(|c| c.0 == *b"cSUM")
        .unwrap();
    let offset = at + 4 + common::varint(&file[at + 4..]).1;
    file[offset] = 127;
    let rec = common::checksum::record_at(&file) + 1;
    file[rec] = 127;
    assert_eq!(
        add_pixel_checksum(file).err(),
        Some(Error::Unsupported("unknown file checksum algorithm"))
    );
}

#[test]
fn sealed_plane_primary_attests_only_primary_and_index_remux_is_refused() {
    use imij::planes::{
        LevelDescriptor, PlaneDescriptor, PlaneSetBuilder, PlaneSetLimits, PlaneSetReader,
    };
    let (info, source) = image(17, 5, Channels::Rgb);
    let primary = add_pixel_checksum(plain(&info, &source, true)).unwrap();
    let mut builder = PlaneSetBuilder::new(&primary).unwrap();
    let id = builder
        .add_plane(PlaneDescriptor::new("auxiliary", SampleType::U8))
        .unwrap();
    let auxiliary = vec![1, 2, 3, 4, 5, 6];
    builder
        .add_level(
            id,
            LevelDescriptor::new(3, 2, 3, 2),
            &auxiliary,
            3,
            imij::ImiEncoderConfig::default(),
        )
        .unwrap();
    let file = builder.finish().unwrap();
    assert_eq!(decode(&file).unwrap().1, source);
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let mut output = vec![0; auxiliary.len()];
    reader
        .decode_region(id, 0, Rect::new(0, 0, 3, 2), &mut output, 3)
        .unwrap();
    assert_eq!(output, auxiliary);
    // A preexisting bitmap table is refused even before plane-index remux.
    assert_eq!(
        add_pixel_checksum(file).err(),
        Some(Error::InvalidInput("file already has pSUM"))
    );
    let primary = plain(&info, &source, false);
    let file = PlaneSetBuilder::new(&primary).unwrap().finish().unwrap();
    assert_eq!(
        add_pixel_checksum(file).err(),
        Some(Error::Unsupported(
            "pSUM insertion requires rebuilding plane indexes"
        ))
    );
}

#[test]
fn sessions_admit_hash_bytes_before_pixels_and_cancel_before_success() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    let (info, source) = image(257, 5, Channels::Rgba);
    let file = add_pixel_checksum(plain(&info, &source, false)).unwrap();
    let mut future = file.clone();
    let at = payload(&future).start;
    future[at] = 2;
    let expected_work =
        (info.width as u64 * info.height as u64 * info.bytes_per_pixel() as u64) + 34;
    for mode in 0..4 {
        let callbacks = AtomicUsize::new(0);
        let run = |session: &mut DecodeSession, data: &[u8], out: &mut [u8]| match mode {
            0 => session.decode(data).map(|_| ()),
            1 => session
                .decode_into(data, out, info.row_bytes() + 7)
                .map(|_| ()),
            2 => session
                .decode_rows(data, out, |_, _, _| {
                    callbacks.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .map(|_| ()),
            _ => session
                .decode_reader(data, |_, _, _| {
                    callbacks.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .map(|_| ()),
        };
        let mut out = vec![0xa5; (info.row_bytes() + 7) * info.height as usize];
        let mut known = DecodeSession::new(DecodePolicy::default());
        run(&mut known, &file, &mut out).unwrap();
        let mut unknown = DecodeSession::new(DecodePolicy::default());
        run(&mut unknown, &future, &mut out).unwrap();
        assert_eq!(
            known.usage().work_units - unknown.usage().work_units,
            expected_work
        );
        let mut limited = DecodeSession::new(
            DecodePolicy::default().with_max_work_units(unknown.usage().work_units),
        );
        out.fill(0xa5);
        callbacks.store(0, Ordering::SeqCst);
        assert_eq!(run(&mut limited, &file, &mut out), Err(Error::TooLarge));
        // Read charges consumed-input work after row delivery. This budget
        // admits hashing, then fails that later charge after all five rows.
        let delivered = if mode == 3 { info.height as usize } else { 0 };
        assert_eq!(callbacks.load(Ordering::SeqCst), delivered);
        assert!(
            out.iter().all(|&b| b == 0xa5),
            "budget before output/callback rows"
        );
    }
    // This separate bound cannot admit even the supported hash charge itself.
    // The reader stops exactly at pSUM, before reading DATA or delivering rows.
    let callbacks = AtomicUsize::new(0);
    let mut limited =
        DecodeSession::new(DecodePolicy::default().with_max_work_units(expected_work - 1));
    assert_eq!(
        limited
            .decode_reader(file.as_slice(), |_, _, _| {
                callbacks.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .err(),
        Some(Error::TooLarge),
    );
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    assert_eq!(limited.usage().input_bytes, payload(&file).end as u64);
    for into in [false, true] {
        let calls = AtomicUsize::new(0);
        let mut probe = DecodeSession::with_cancel(DecodePolicy::default(), || {
            calls.fetch_add(1, Ordering::SeqCst);
            false
        });
        let stride = info.row_bytes() + 7;
        let mut out = vec![0xa5; stride * info.height as usize];
        if into {
            probe.decode_into(&file, &mut out, stride).unwrap();
        } else {
            probe.decode(&file).unwrap();
        }
        let total = calls.load(Ordering::SeqCst);
        calls.store(0, Ordering::SeqCst);
        let mut cancelled = DecodeSession::with_cancel(DecodePolicy::default(), || {
            calls.fetch_add(1, Ordering::SeqCst) + 1 == total
        });
        let result = if into {
            cancelled.decode_into(&file, &mut out, stride).map(|_| ())
        } else {
            cancelled.decode(&file).map(|_| ())
        };
        assert_eq!(result, Err(Error::Cancelled));
        if into {
            for (row, expected) in out.chunks(stride).zip(source.chunks(info.row_bytes())) {
                assert_eq!(
                    &row[..info.row_bytes()],
                    expected,
                    "cancel after reconstruction"
                );
                assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xa5));
            }
        }
    }
    for reader in [false, true] {
        use core::sync::atomic::AtomicBool;
        let stop = AtomicBool::new(false);
        let mut session =
            DecodeSession::with_cancel(DecodePolicy::default(), || stop.load(Ordering::SeqCst));
        let mut row = vec![0; info.row_bytes()];
        let on_row = |_: &ImageInfo, y, _: &[u8]| {
            if y + 1 == info.height {
                stop.store(true, Ordering::SeqCst);
            }
            Ok(())
        };
        let result = if reader {
            session.decode_reader(file.as_slice(), on_row).map(|_| ())
        } else {
            session.decode_rows(&file, &mut row, on_row).map(|_| ())
        };
        assert_eq!(result, Err(Error::Cancelled));
    }
}
