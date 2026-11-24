use enough::Unstoppable;
use imij_png::{TranscodeConfig, transcode};

// Small independent fixture writer: uncompressed zlib blocks and explicit PNG
// records. Production decoding/encoding always uses zenpng's public APIs.
fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(data);
    let mut crc = !0u32;
    for &b in tag.iter().chain(data) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}
fn zlib(raw: &[u8]) -> Vec<u8> {
    assert!(raw.len() <= u16::MAX as usize);
    let mut out = vec![0x78, 0x01, 1];
    out.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    out.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    out.extend_from_slice(raw);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    out.extend_from_slice(&((b << 16) | a).to_be_bytes());
    out
}
fn header(w: u32, h: u32, depth: u8, color: u8, interlace: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::from(w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[depth, color, 0, 0, interlace]);
    chunk(&mut out, b"IHDR", &ihdr);
    out
}
fn png(
    w: u32,
    h: u32,
    depth: u8,
    color: u8,
    interlace: u8,
    scan: &[u8],
    meta: &[([u8; 4], Vec<u8>)],
) -> Vec<u8> {
    let mut out = header(w, h, depth, color, interlace);
    for (tag, bytes) in meta {
        chunk(&mut out, tag, bytes);
    }
    chunk(&mut out, b"IDAT", &zlib(scan));
    chunk(&mut out, b"IEND", &[]);
    out
}
fn scan(raw: &[u8], row: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for r in raw.chunks(row) {
        out.push(0);
        out.extend_from_slice(r);
    }
    out
}
fn config() -> TranscodeConfig {
    TranscodeConfig::default().with_imij({
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(0.0);
        cfg = cfg.with_threads(1);
        cfg
    })
}
fn check(bytes: &[u8]) -> imij_png::TranscodeOutput {
    let png = zenpng::decode(bytes, &zenpng::PngDecodeConfig::default(), &Unstoppable).unwrap();
    let result = config()
        .job()
        .transcoder(bytes)
        .unwrap()
        .transcode()
        .unwrap();
    assert!(result.fallback.is_none());
    let (info, raw) = imij::decode(&result.image).unwrap();
    assert_eq!(
        (info.width, info.height),
        (png.pixels.width(), png.pixels.height())
    );
    let oracle = imij_oracle::decode(&result.image).unwrap();
    let mut little = Vec::new();
    for y in 0..info.height {
        let row = png.pixels.as_slice().row(y).to_vec();
        assert_eq!(
            &raw[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()],
            row
        );
        if info.sample == imij::SampleType::U16 {
            for value in row.as_chunks::<2>().0 {
                little.extend_from_slice(&u16::from_ne_bytes(*value).to_le_bytes());
            }
        } else {
            little.extend_from_slice(&row);
        }
    }
    assert_eq!(oracle.frames, vec![little]);
    result
}
#[test]
fn all_png_storage_models_palette_transparency_and_low_bits() {
    for (color, channels) in [(0, 1), (2, 3), (4, 2), (6, 4)] {
        for depth in [8, 16] {
            let raw: Vec<u8> = (0..7 * 3 * channels * (depth / 8) as usize)
                .map(|i| (i * 29 + i / 11) as u8)
                .collect();
            let row = 7 * channels * (depth / 8) as usize;
            let source = png(7, 3, depth, color, 0, &scan(&raw, row), &[]);
            check(&source);
            if color == 0 || color == 2 {
                let transparency = if depth == 16 {
                    raw[..channels * 2].to_vec()
                } else {
                    raw[..channels].iter().flat_map(|&v| [0, v]).collect()
                };
                check(&png(
                    7,
                    3,
                    depth,
                    color,
                    0,
                    &scan(&raw, row),
                    &[(*b"tRNS", transparency)],
                ));
            }
        }
    }
    for depth in [1u8, 2, 4, 8] {
        let count = 1usize << depth;
        let palette: Vec<u8> = (0..count)
            .flat_map(|i| [i as u8, (i * 37) as u8, (i * 71) as u8])
            .collect();
        let mut packed = vec![0u8; (13 * depth as usize).div_ceil(8)];
        for x in 0..13 {
            let bit = x * depth as usize;
            packed[bit / 8] |= ((x % count) as u8) << (8 - depth as usize - bit % 8);
        }
        let rows = scan(&packed, packed.len());
        for alpha in [false, true] {
            let mut meta = vec![(*b"PLTE", palette.clone())];
            if alpha {
                meta.push((
                    *b"tRNS",
                    (0..count).map(|i| (i * 255 / (count - 1)) as u8).collect(),
                ));
            }
            let source = png(13, 1, depth, 3, 0, &rows, &meta);
            let result = check(&source);
            let (_, rgba) = imij::decode(&result.image).unwrap();
            let channels = if alpha { 4 } else { 3 };
            for x in 0..13 {
                assert_eq!(
                    &rgba[x * channels..x * channels + 3],
                    &palette[(x % count) * 3..(x % count) * 3 + 3]
                );
            }
        }
        if depth < 8 {
            check(&png(13, 1, depth, 0, 0, &rows, &[]));
        }
    }
}
#[test]
fn adam7_odd_edges_and_exact_hdr_orientation() {
    let (w, h) = (9, 7);
    let raw: Vec<u8> = (0..w * h * 3).map(|i| (i * 37) as u8).collect();
    let mut passes = Vec::new();
    for (x0, y0, dx, dy) in [
        (0, 0, 8, 8),
        (4, 0, 8, 8),
        (0, 4, 4, 8),
        (2, 0, 4, 4),
        (0, 2, 2, 4),
        (1, 0, 2, 2),
        (0, 1, 1, 2),
    ] {
        for y in (y0..h).step_by(dy) {
            if x0 < w {
                passes.push(0);
                for x in (x0..w).step_by(dx) {
                    passes.extend_from_slice(&raw[(y * w + x) * 3..(y * w + x) * 3 + 3]);
                }
            }
        }
    }
    let mut exif = zencodec::exif::Exif::default();
    exif.set_orientation(zencodec::Orientation::Rotate90);
    let cll = [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 1];
    let mut mdcv = vec![0; 24];
    mdcv[0..2].copy_from_slice(&32000u16.to_be_bytes());
    mdcv[16..20].copy_from_slice(&10000000u32.to_be_bytes());
    let source = png(
        w as u32,
        h as u32,
        8,
        2,
        1,
        &passes,
        &[
            (*b"cICP", vec![9, 16, 0, 1]),
            (*b"cLLI", cll.to_vec()),
            (*b"mDCV", mdcv.clone()),
            (*b"eXIf", exif.to_bytes()),
        ],
    );
    let out = check(&source);
    let (info, pixels) = imij::decode(&out.image).unwrap();
    assert_eq!(pixels, raw);
    assert!(out.source.interlaced);
    assert_eq!(info.cicp.transfer_characteristics, 16);
    let (_, meta) = imij::probe(&out.image).unwrap();
    assert_eq!(meta.orientation, 6);
    assert_eq!(meta.content_light_level, Some(cll));
    assert_eq!(meta.mastering_display, Some(mdcv.try_into().unwrap()));
}

#[allow(clippy::too_many_arguments)] // Explicit fcTL wire fields in this fixture writer.
fn fctl(
    out: &mut Vec<u8>,
    seq: &mut u32,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
    delay: (u16, u16),
    dispose: u8,
    blend: u8,
) {
    let mut p = Vec::new();
    for n in [*seq, w, h, x, y] {
        p.extend_from_slice(&n.to_be_bytes());
    }
    *seq += 1;
    p.extend_from_slice(&delay.0.to_be_bytes());
    p.extend_from_slice(&delay.1.to_be_bytes());
    p.extend_from_slice(&[dispose, blend]);
    chunk(out, b"fcTL", &p);
}
fn fdat(out: &mut Vec<u8>, seq: &mut u32, raw: &[u8]) {
    let mut p = Vec::from(seq.to_be_bytes());
    *seq += 1;
    p.extend_from_slice(&zlib(raw));
    chunk(out, b"fdAT", &p);
}
fn apng(depth: u8, fallback: bool) -> Vec<u8> {
    let mut out = header(5, 3, depth, 6, 0);
    let mut actl = Vec::from(4u32.to_be_bytes());
    actl.extend_from_slice(&5u32.to_be_bytes());
    chunk(&mut out, b"acTL", &actl);
    let sample = |rgba: [u16; 4], count: usize| -> Vec<u8> {
        (0..count)
            .flat_map(|_| rgba)
            .flat_map(|n| {
                if depth == 16 {
                    n.to_be_bytes().to_vec()
                } else {
                    vec![(n >> 8) as u8]
                }
            })
            .collect::<Vec<_>>()
    };
    let blue = sample([0, 0, 65535, 65535], 15);
    let row = 5 * 4 * (depth as usize / 8);
    let mut seq = 0;
    if fallback {
        chunk(
            &mut out,
            b"IDAT",
            &zlib(&scan(&sample([65535, 0, 65535, 65535], 15), row)),
        );
    }
    fctl(&mut out, &mut seq, 5, 3, 0, 0, (1, 0), 0, 0);
    if fallback {
        fdat(&mut out, &mut seq, &scan(&blue, row));
    } else {
        chunk(&mut out, b"IDAT", &zlib(&scan(&blue, row)));
    }
    let red = sample([65535, 0, 0, 32768], 2);
    fctl(&mut out, &mut seq, 2, 1, 1, 1, (2, 3), 2, 1);
    fdat(&mut out, &mut seq, &scan(&red, red.len()));
    let green = sample([0, 65535, 0, 65535], 1);
    fctl(&mut out, &mut seq, 1, 1, 0, 0, (0, 1), 1, 0);
    fdat(&mut out, &mut seq, &scan(&green, green.len()));
    let white = sample([65535; 4], 1);
    fctl(&mut out, &mut seq, 1, 1, 4, 2, (5, 1000), 0, 0);
    fdat(&mut out, &mut seq, &scan(&white, white.len()));
    chunk(&mut out, b"IEND", &[]);
    out
}
#[test]
fn apng_canvases_disposals_blend_timing_and_distinct_fallback() {
    for depth in [8, 16] {
        for fallback in [false, true] {
            let input = apng(depth, fallback);
            let reference =
                zenpng::decode_apng(&input, &zenpng::PngDecodeConfig::default(), &Unstoppable)
                    .unwrap();
            let poster =
                zenpng::decode(&input, &zenpng::PngDecodeConfig::default(), &Unstoppable).unwrap();
            let out = transcode(&input, &config(), &Unstoppable).unwrap();
            let still = imij::decode(out.fallback.as_ref().unwrap()).unwrap().1;
            for y in 0..3 {
                assert_eq!(
                    &still[y * poster.pixels.stride()..(y + 1) * poster.pixels.stride()],
                    poster.pixels.as_slice().row(y as u32)
                );
            }
            let oracle = imij_oracle::decode(&out.image).unwrap();
            assert_eq!(oracle.frames.len(), 4);
            let mut native = imij::FrameDecoder::new(&out.image).unwrap();
            assert_eq!(native.loop_count(), 5);
            let info = native.info().clone();
            let mut canvas = vec![0; info.checked_buffer_len().unwrap()];
            for (index, frame) in reference.frames.iter().enumerate() {
                let h = native.next_frame().unwrap().unwrap();
                let d = frame.frame_info;
                assert_eq!(
                    (h.duration.num, h.duration.den),
                    (
                        u64::from(d.delay_num),
                        u32::from(if d.delay_den == 0 { 100 } else { d.delay_den })
                    )
                );
                native
                    .decode_into(&mut canvas, info.row_bytes(), 1)
                    .unwrap();
                let mut little = Vec::new();
                for y in 0..3 {
                    let row = frame.pixels.as_slice().row(y).to_vec();
                    assert_eq!(
                        &canvas[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()],
                        row
                    );
                    if depth == 16 {
                        for p in row.as_chunks::<2>().0 {
                            little.extend_from_slice(&u16::from_ne_bytes(*p).to_le_bytes());
                        }
                    } else {
                        little.extend_from_slice(&row);
                    }
                }
                assert_eq!(oracle.frames[index], little);
            }
            assert!(native.next_frame().unwrap().is_none());
            let bpp = 4 * (depth as usize / 8);
            // PREVIOUS restores blue before frame 2; BACKGROUND clears frame 2's pixel before frame 3.
            assert_eq!(
                &oracle.frames[2][(5 + 1) * bpp..(5 + 2) * bpp],
                &oracle.frames[0][(5 + 1) * bpp..(5 + 2) * bpp]
            );
            assert!(oracle.frames[3][..bpp].iter().all(|&b| b == 0));
            if fallback {
                assert_ne!(still, imij::decode(&out.image).unwrap().1);
            }
        }
    }
}
#[test]
fn precision_policy_limits_and_cancellation_are_explicit() {
    let raw = vec![0, 37, 128, 255];
    let scan = scan(&raw, 4);
    let source = png(4, 1, 8, 0, 0, &scan, &[(*b"sBIT", vec![5])]);
    let out = check(&source);
    assert_eq!(imij::decode(&out.image).unwrap().1, raw);
    assert_eq!(imij::probe(&out.image).unwrap().0.significant_bits, [0; 4]);
    for bits in [vec![0], vec![9], vec![1, 2, 3]] {
        assert!(
            transcode(
                &png(4, 1, 8, 0, 0, &scan, &[(*b"sBIT", bits)]),
                &config(),
                &Unstoppable
            )
            .is_err()
        );
    }
    let limited = config().with_png(zenpng::PngDecodeConfig::default().with_max_pixels(3));
    assert!(transcode(&source, &limited, &Unstoppable).is_err());
    let lossy = config().with_imij({
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.max_error = [1; 4];
        });
        cfg
    });
    assert!(transcode(&source, &lossy, &Unstoppable).is_err());
    let legacy = png(
        4,
        1,
        8,
        0,
        0,
        &scan,
        &[(*b"gAMA", 50000u32.to_be_bytes().to_vec())],
    );
    assert!(transcode(&legacy, &config(), &Unstoppable).is_err());
    struct Cancel;
    impl enough::Stop for Cancel {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }
    assert!(matches!(
        transcode(&source, &config(), &Cancel),
        Err(imij_png::TranscodeError::Stopped(
            enough::StopReason::Cancelled
        ))
    ));
}

#[test]
fn zencodec_job_limits_and_owned_cancellation() {
    use zencodec::{ResourceLimits, StopToken, ThreadingPolicy};
    let source = png(2, 2, 8, 0, 0, &scan(&[1, 2, 3, 4], 2), &[]);
    for limits in [
        ResourceLimits::default().with_max_input_bytes(1),
        ResourceLimits::default().with_max_width(1),
        ResourceLimits::default().with_max_height(1),
        ResourceLimits::default().with_max_pixels(3),
        ResourceLimits::default().with_max_total_pixels(3),
        ResourceLimits::default().with_max_frames(0),
        ResourceLimits::default().with_max_output(1),
    ] {
        let result = config()
            .job()
            .with_limits(limits)
            .transcoder(&source)
            .and_then(|executor| executor.transcode());
        assert!(matches!(result, Err(imij_png::TranscodeError::Limit(_))));
    }
    assert!(matches!(
        config()
            .job()
            .with_limits(ResourceLimits::default().with_max_memory(4096))
            .transcoder(&source),
        Err(imij_png::TranscodeError::Imij(imij::Error::Unsupported(_)))
    ));
    let reusable = config();
    let result = reusable
        .clone()
        .job()
        .with_stop(StopToken::new(Unstoppable))
        .with_limits(
            ResourceLimits::default()
                .with_max_pixels(4)
                .with_max_total_pixels(4)
                .with_max_frames(1)
                .with_threading(ThreadingPolicy::Sequential),
        )
        .transcoder(&source)
        .unwrap()
        .transcode()
        .unwrap();
    assert_eq!(imij::decode(&result.image).unwrap().1, [1, 2, 3, 4]);
    let animation = apng(8, true);
    for limits in [
        ResourceLimits::default().with_max_frames(3),
        ResourceLimits::default().with_max_total_pixels(59),
        ResourceLimits::default().with_max_animation_ms(9),
    ] {
        assert!(matches!(
            reusable
                .clone()
                .job()
                .with_limits(limits)
                .transcoder(&animation)
                .unwrap()
                .transcode(),
            Err(imij_png::TranscodeError::Limit(_))
        ));
    }
    struct Cancel;
    impl enough::Stop for Cancel {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }
    assert!(matches!(
        reusable
            .job()
            .with_stop(StopToken::new(Cancel))
            .transcoder(&source),
        Err(imij_png::TranscodeError::Stopped(
            enough::StopReason::Cancelled
        ))
    ));
}

#[test]
fn job_keeps_timeout_reason() {
    struct Timeout;
    impl enough::Stop for Timeout {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::TimedOut)
        }
    }
    assert!(matches!(
        config()
            .job()
            .with_stop(zencodec::StopToken::new(Timeout))
            .transcoder(&[]),
        Err(imij_png::TranscodeError::Stopped(
            enough::StopReason::TimedOut
        ))
    ));
}

#[test]
fn unknown_safe_ancillary_chunks_keep_idat_side_unsafe_dropped() {
    // `vpAg`/`prVa` are private safe-to-copy names (lowercase fourth byte);
    // `vpAG` is the unsafe twin that PNG-3 forbids copying after critical
    // image data changes, which transcoding always does.
    let raw: Vec<u8> = (0..7 * 3 * 3).map(|i| (i * 31 + 7) as u8).collect();
    let mut source = header(7, 3, 8, 2, 0);
    chunk(&mut source, b"vpAg", b"pre-idat-value");
    chunk(&mut source, b"vpAG", b"unsafe-payload");
    chunk(&mut source, b"IDAT", &zlib(&scan(&raw, 21)));
    chunk(&mut source, b"prVa", b"post-idat-value");
    chunk(&mut source, b"IEND", &[]);

    let result = check(&source); // exact pixels + oracle frames already verified
    let decoder = imij::FrameDecoder::new(&result.image).unwrap();
    let stored: Vec<(&[u8; 4], &[u8])> = decoder
        .metadata()
        .chunks
        .iter()
        .map(|c| (&c.tag, c.data.as_slice()))
        .collect();
    // `cORg` is IMI's color-provenance record; it rides `chunks` first and
    // is never emitted to PNG on export.
    assert_eq!(
        stored,
        [
            (b"cORg", [1, 0, 0, 0].as_slice()),
            (b"vpAg", b"pre-idat-value".as_slice()),
            (b"idat", b"".as_slice()),
            (b"prVa", b"post-idat-value".as_slice())
        ]
    );

    // Re-encode to PNG: both safe chunks reappear verbatim on their original
    // sides of IDAT; the unsafe chunk and the internal `idat` marker do not.
    let bytes = imij_png::ExportConfig::default()
        .job()
        .exporter(&result.image)
        .unwrap()
        .export()
        .unwrap();
    let exported = zenpng::decode(
        &bytes,
        &zenpng::PngDecodeConfig::default().with_collect_unknown_chunks(true),
        &Unstoppable,
    )
    .unwrap();
    assert_eq!(
        exported.info.unknown_chunks,
        [
            zenpng::OwnedAncillaryChunk {
                tag: *b"vpAg",
                data: b"pre-idat-value".to_vec(),
                position: zenpng::AncillaryPosition::BeforeIdat,
            },
            zenpng::OwnedAncillaryChunk {
                tag: *b"prVa",
                data: b"post-idat-value".to_vec(),
                position: zenpng::AncillaryPosition::AfterIdat,
            }
        ]
    );
    for y in 0..3u32 {
        assert_eq!(
            exported.pixels.as_slice().row(y).to_vec(),
            raw[y as usize * 21..(y as usize + 1) * 21]
        );
    }
}

#[test]
fn interpreted_chunks_are_not_duplicated_through_unknown_retention() {
    // eXIf is parsed into `Metadata::exif` and re-emitted by the encoder
    // from that typed field; it must not also ride the unknown-chunk list
    // or the exported PNG would carry it twice.
    let raw: Vec<u8> = (0..7 * 3 * 3).map(|i| (i * 17 + 3) as u8).collect();
    // Minimal well-formed EXIF: little-endian TIFF header pointing at an
    // IFD with zero entries.
    let mut exif = b"II".to_vec();
    exif.extend_from_slice(&42u16.to_le_bytes());
    exif.extend_from_slice(&8u32.to_le_bytes());
    exif.extend_from_slice(&0u16.to_le_bytes());
    exif.extend_from_slice(&0u32.to_le_bytes());
    let source = png(7, 3, 8, 2, 0, &scan(&raw, 21), &[(*b"eXIf", exif)]);
    let result = check(&source);
    let decoder = imij::FrameDecoder::new(&result.image).unwrap();
    assert!(decoder.metadata().chunks.iter().all(|c| c.tag != *b"eXIf"));
    let bytes = imij_png::ExportConfig::default()
        .job()
        .exporter(&result.image)
        .unwrap()
        .export()
        .unwrap();
    let count = zenpng::ancillary_chunks(&bytes, false)
        .unwrap()
        .filter(|c| c.as_ref().map(|c| &c.tag == b"eXIf").unwrap_or(false))
        .count();
    assert_eq!(count, 1);
}
