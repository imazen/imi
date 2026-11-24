use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{Channels, ImageInfo, ImiEncoderConfig, RowGroups, SampleType};
use alloc::vec;
use std::io::{self, Cursor, Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

fn spatial() -> SpatialConfig {
    SpatialConfig {
        tile_columns: 3,
        tile_rows: 2,
        order: TileOrder::Morton,
        integrity: ObjectIntegrity::ObjectsAndTiles,
        limits: TileLimits {
            max_object_bytes: 1 << 20,
            max_tile_bytes: 16 << 20,
        },
    }
}
fn config() -> ImiEncoderConfig {
    ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(9, 7, 8 << 20))
}
fn fill<W: Write, S: Read + Write + std::io::Seek>(
    encoder: &mut NumericFileEncoder<W, S>,
    pixels: &[u8],
    stride: usize,
    bpp: usize,
) {
    for _ in 0..encoder.layout().tile_count() {
        encoder.begin_tile().unwrap();
        while let Some(rect) = encoder.next_group() {
            encoder
                .push_group(
                    &pixels[rect.y as usize * stride + rect.x as usize * bpp..],
                    stride,
                )
                .unwrap();
        }
        encoder.end_tile().unwrap();
    }
}

#[test]
fn forward_file_matches_allocating_file_and_preserves_scratch_prefix() {
    for quick in [false, true] {
        let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U16);
        let stride = info.row_bytes() + 13;
        let pixels: Vec<u8> = (0..stride * info.height as usize)
            .map(|i| (i * 73 + i / 7 * 41) as u8)
            .collect();
        let cfg = if quick {
            ImiEncoderConfig::quick().with_row_groups(RowGroups::new(9, 7, 8 << 20))
        } else {
            config()
        };
        let meta = crate::Metadata::default();
        let expected =
            encode_numeric(info.clone(), &pixels, stride, &meta, &cfg, spatial()).unwrap();
        let mut scratch = Cursor::new(vec![7, 8, 9]);
        scratch.set_position(1);
        let mut encoder = NumericFileEncoder::new(
            info.clone(),
            &meta,
            &cfg,
            spatial(),
            Vec::new(),
            scratch,
            1 << 20,
            1 << 24,
        )
        .unwrap();
        fill(&mut encoder, &pixels, stride, info.bytes_per_pixel());
        let (file, scratch) = encoder.finish().unwrap();
        assert_eq!(&scratch.get_ref()[..3], &[7, 8, 9]);
        assert_eq!(
            file,
            super::file_tests::rewrite_placement(&expected, DirectoryPlacement::Tail, true)
        );
        let session = DecodeSession::new(DecodePolicy::default());
        let decoder = NumericFile::open(&file, spatial().limits, &session).unwrap();
        let mut output = vec![0xdc; pixels.len()];
        decoder.decode_into(&mut output, stride).unwrap();
        for y in 0..info.height as usize {
            assert_eq!(
                &output[y * stride..y * stride + info.row_bytes()],
                &pixels[y * stride..y * stride + info.row_bytes()]
            );
            assert!(
                output[y * stride + info.row_bytes()..(y + 1) * stride]
                    .iter()
                    .all(|&v| v == 0xdc)
            );
        }
    }
}

#[derive(Clone)]
struct FallibleWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: Arc<AtomicBool>,
}
impl Write for FallibleWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(io::Error::other("destination failure"));
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Unreadable(Cursor<Vec<u8>>);
impl Read for Unreadable {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("scratch read"))
    }
}
impl Write for Unreadable {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl std::io::Seek for Unreadable {
    fn seek(&mut self, p: std::io::SeekFrom) -> io::Result<u64> {
        self.0.seek(p)
    }
}

#[test]
fn errors_poison_writer_and_never_report_finalized_success() {
    let info = ImageInfo::new(41, 31, Channels::Gray, SampleType::U8);
    let cfg = config();
    let meta = crate::Metadata::default();
    let writer = FallibleWriter {
        bytes: Arc::new(Mutex::new(Vec::new())),
        fail: Arc::new(AtomicBool::new(false)),
    };
    let mut encoder = NumericFileEncoder::new(
        info.clone(),
        &meta,
        &cfg,
        spatial(),
        writer.clone(),
        Cursor::new(Vec::new()),
        1 << 20,
        1 << 24,
    )
    .unwrap();
    encoder.begin_tile().unwrap();
    writer.fail.store(true, Ordering::Relaxed);
    assert!(matches!(
        encoder.push_group(&[0; 9 * 7], 9),
        Err(crate::stream_encode::StreamEncodeError::Write(_))
    ));
    let published = writer.bytes.lock().unwrap().len();
    writer.fail.store(false, Ordering::Relaxed);
    assert!(encoder.push_group(&[0; 9 * 7], 9).is_err());
    assert!(encoder.end_tile().is_err());
    assert!(encoder.finish().is_err());
    assert_eq!(writer.bytes.lock().unwrap().len(), published);
    let mut destination = Vec::new();
    assert!(
        NumericFileEncoder::new(
            info.clone(),
            &meta,
            &cfg,
            spatial(),
            &mut destination,
            Cursor::new(Vec::new()),
            1 << 20,
            0
        )
        .is_err()
    );
    assert!(destination.is_empty());
    let mut encoder = NumericFileEncoder::new(
        info.clone(),
        &meta,
        &cfg,
        spatial(),
        Vec::new(),
        Cursor::new(Vec::new()),
        0,
        1 << 24,
    )
    .unwrap();
    assert!(encoder.begin_tile().is_err());
    assert!(encoder.finish().is_err());
    let mut encoder = NumericFileEncoder::new(
        info.clone(),
        &meta,
        &cfg,
        spatial(),
        Vec::new(),
        Unreadable(Cursor::new(Vec::new())),
        1 << 20,
        1 << 24,
    )
    .unwrap();
    fill(
        &mut encoder,
        &vec![0; info.row_bytes() * info.height as usize],
        info.row_bytes(),
        1,
    );
    assert!(matches!(
        encoder.finish(),
        Err(crate::stream_encode::StreamEncodeError::Read(_))
    ));
}

#[cfg(unix)]
#[test]
fn real_pipe_delivers_decoded_group_before_next_input_or_finalization() {
    use std::{
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        loop {
            let n = stdout.read(&mut buffer).unwrap();
            if tx.send(buffer[..n].to_vec()).is_err() || n == 0 {
                break;
            }
        }
    });
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    let mut encoder = NumericFileEncoder::new(
        info.clone(),
        &Default::default(),
        &config(),
        spatial(),
        child.0.stdin.take().unwrap(),
        Cursor::new(Vec::new()),
        1 << 20,
        1 << 24,
    )
    .unwrap();
    encoder.begin_tile().unwrap();
    let rect = encoder.next_group().unwrap();
    encoder.push_group(&pixels, info.row_bytes()).unwrap();
    let mut received = Vec::new();
    while received.len() < encoder.bytes_written() as usize {
        let chunk = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("first-group pipe delivery");
        assert!(!chunk.is_empty(), "pipe closed before the next input");
        received.extend_from_slice(&chunk);
    }
    assert!(child.0.try_wait().unwrap().is_none());
    let mut outer = crate::ByteReader::new(&received);
    let head = crate::parse::header::read_header_v4(&mut outer, 64 << 10).unwrap();
    let (tag, len) = crate::parse::chunks::read_head(&mut outer).unwrap();
    assert_eq!(&tag, b"DATA");
    assert_eq!(len, None);
    let payload = &received[outer.pos..];
    let session = DecodeSession::new(DecodePolicy::default());
    let (mut decoder, first) = NumericGroupDecoder::new(
        head.header.info,
        head.declared,
        payload,
        spatial().limits,
        &session,
    )
    .unwrap();
    let begin = decoder.read(&payload[first..]).unwrap().entry();
    let NumericEvent::Group(work) = decoder
        .read(&payload[first + begin.entry.record_len as usize..])
        .unwrap()
    else {
        panic!("first numeric group");
    };
    let local_stride = rect.width as usize * 4;
    let mut rows = vec![0; local_stride * rect.height as usize];
    work.decode_into(&mut rows, local_stride, &session).unwrap();
    for y in 0..rect.height as usize {
        assert_eq!(
            &rows[y * local_stride..(y + 1) * local_stride],
            &pixels[y * info.row_bytes()..y * info.row_bytes() + local_stride]
        );
    }
    // The next input is deliberately not supplied until decoded rows exist.
    while let Some(rect) = encoder.next_group() {
        encoder
            .push_group(
                &pixels[rect.y as usize * info.row_bytes() + rect.x as usize * 4..],
                info.row_bytes(),
            )
            .unwrap();
    }
    encoder.end_tile().unwrap();
    for _ in 1..encoder.layout().tile_count() {
        encoder.begin_tile().unwrap();
        while let Some(rect) = encoder.next_group() {
            encoder
                .push_group(
                    &pixels[rect.y as usize * info.row_bytes() + rect.x as usize * 4..],
                    info.row_bytes(),
                )
                .unwrap();
        }
        encoder.end_tile().unwrap();
    }
    let (stdin, _) = encoder.finish().unwrap();
    drop(stdin);
    loop {
        let chunk = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("final pipe delivery");
        if chunk.is_empty() {
            break;
        }
        received.extend_from_slice(&chunk);
    }
    assert!(child.0.wait().unwrap().success());
    reader.join().unwrap();
    let session = DecodeSession::new(DecodePolicy::default());
    let mut output = vec![0; pixels.len()];
    NumericFile::open(&received, spatial().limits, &session)
        .unwrap()
        .decode_into(&mut output, info.row_bytes())
        .unwrap();
    assert_eq!(output, pixels);
}

#[test]
fn checked_forward_files_preserve_pixels_scratch_prefixes_and_sparse_checksums() {
    use crate::stream_encode::ChecksummedGroupConfig;
    for quick in [false, true] {
        for algorithm in [
            crate::HashAlgorithm::Xxh3_64,
            crate::HashAlgorithm::Xxh3_128,
        ] {
            for piece_log2 in [10, 20] {
                let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U16);
                let stride = info.row_bytes() + 13;
                let pixels: Vec<u8> = (0..stride * info.height as usize)
                    .map(|i| (i * 73 + i / 7 * 41) as u8)
                    .collect();
                let packed: Vec<u8> = pixels
                    .chunks(stride)
                    .flat_map(|row| row[..info.row_bytes()].iter().copied())
                    .collect();
                let cfg = if quick {
                    ImiEncoderConfig::quick().with_row_groups(config().row_groups())
                } else {
                    config()
                }
                .with_hash_algorithm(algorithm);
                let meta = crate::Metadata {
                    xmp: Some(b"<checked-forward/>".to_vec()),
                    ..Default::default()
                };
                let mut encoder = ChecksummedNumericFileEncoder::new(
                    info.clone(),
                    &meta,
                    spatial(),
                    Vec::new(),
                    Cursor::new(vec![7, 8, 9]),
                    Cursor::new(vec![11, 13]),
                    ChecksummedGroupConfig::new(cfg, 1 << 20, piece_log2, 1 << 20),
                    1 << 24,
                )
                .unwrap();
                for _ in 0..encoder.layout().tile_count() {
                    encoder.begin_tile().unwrap();
                    while let Some(rect) = encoder.next_group() {
                        encoder
                            .push_group(
                                &pixels[rect.y as usize * stride
                                    + rect.x as usize * info.bytes_per_pixel()..],
                                stride,
                            )
                            .unwrap();
                    }
                    encoder.end_tile().unwrap();
                }
                let (file, index, checksums) = encoder.finish().unwrap();
                assert_eq!(&index.get_ref()[..3], &[7, 8, 9]);
                assert_eq!(&checksums.get_ref()[..2], &[11, 13]);
                let session = DecodeSession::new(DecodePolicy::default());
                let parsed = super::file::parse_file(&file, &session).unwrap();
                assert!(parsed.checksum.is_some());
                let mut output = vec![0xa7; pixels.len()];
                NumericFile::open(&file, spatial().limits, &session)
                    .unwrap()
                    .decode_into(&mut output, stride)
                    .unwrap();
                for y in 0..info.height as usize {
                    assert_eq!(
                        &output[y * stride..y * stride + info.row_bytes()],
                        &pixels[y * stride..y * stride + info.row_bytes()]
                    );
                    assert!(
                        output[y * stride + info.row_bytes()..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0xa7)
                    );
                }
                super::region_tests::check(&file, &packed, &info);
                super::feed_tests::check_stream(&file, &packed, &info);
                let mut corrupt = file;
                let at = corrupt
                    .windows(b"<checked-forward/>".len())
                    .position(|bytes| bytes == b"<checked-forward/>")
                    .unwrap();
                corrupt[at] ^= 1;
                assert_eq!(crate::decode(&corrupt), Err(crate::checksum::MISMATCH));
            }
        }
    }
}

#[test]
fn checked_forward_failures_are_sticky_and_preserve_io_error_kinds() {
    use crate::stream_encode::{ChecksummedGroupConfig, StreamEncodeError};
    let info = ImageInfo::new(41, 31, Channels::Gray, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    let cfg = || ChecksummedGroupConfig::new(config(), 1 << 20, 10, 1 << 20);
    let mut output = Vec::new();
    assert!(
        ChecksummedNumericFileEncoder::new(
            info.clone(),
            &Default::default(),
            spatial(),
            &mut output,
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            cfg(),
            0
        )
        .is_err()
    );
    assert!(output.is_empty());

    let writer = FallibleWriter {
        bytes: Arc::new(Mutex::new(Vec::new())),
        fail: Arc::new(AtomicBool::new(false)),
    };
    let mut encoder = ChecksummedNumericFileEncoder::new(
        info.clone(),
        &Default::default(),
        spatial(),
        writer.clone(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        cfg(),
        1 << 24,
    )
    .unwrap();
    encoder.begin_tile().unwrap();
    writer.fail.store(true, Ordering::Relaxed);
    assert!(matches!(
        encoder.push_group(&pixels, info.row_bytes()),
        Err(StreamEncodeError::Write(_))
    ));
    let written = writer.bytes.lock().unwrap().len();
    writer.fail.store(false, Ordering::Relaxed);
    assert!(encoder.push_group(&pixels, info.row_bytes()).is_err());
    assert!(encoder.end_tile().is_err());
    assert!(encoder.finish().is_err());
    assert_eq!(writer.bytes.lock().unwrap().len(), written);

    let mut encoder = ChecksummedNumericFileEncoder::new(
        info.clone(),
        &Default::default(),
        spatial(),
        Vec::new(),
        Cursor::new(Vec::new()),
        Unreadable(Cursor::new(Vec::new())),
        cfg(),
        1 << 24,
    )
    .unwrap();
    for _ in 0..encoder.layout().tile_count() {
        encoder.begin_tile().unwrap();
        while let Some(rect) = encoder.next_group() {
            encoder
                .push_group(
                    &pixels[rect.y as usize * info.row_bytes() + rect.x as usize..],
                    info.row_bytes(),
                )
                .unwrap();
        }
        encoder.end_tile().unwrap();
    }
    assert!(matches!(encoder.finish(), Err(StreamEncodeError::Read(_))));

    let mut limited = cfg();
    limited.max_checksum_bytes = 16;
    let mut encoder = ChecksummedNumericFileEncoder::new(
        info.clone(),
        &Default::default(),
        spatial(),
        Vec::new(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        limited,
        1 << 24,
    )
    .unwrap();
    let mut denied = false;
    'tiles: for _ in 0..encoder.layout().tile_count() {
        if encoder.begin_tile().is_err() {
            denied = true;
            break;
        }
        while let Some(rect) = encoder.next_group() {
            if let Err(error) = encoder.push_group(
                &pixels[rect.y as usize * info.row_bytes() + rect.x as usize..],
                info.row_bytes(),
            ) {
                assert!(matches!(
                    error,
                    StreamEncodeError::Codec(crate::Error::TooLarge)
                ));
                denied = true;
                break 'tiles;
            }
        }
        if encoder.end_tile().is_err() {
            denied = true;
            break;
        }
    }
    assert!(
        denied,
        "fixture must exhaust checksum scratch before completion"
    );
    assert!(encoder.next_group().is_none());
    assert!(encoder.finish().is_err());
}

#[cfg(unix)]
#[test]
fn checked_pipe_delivers_rows_before_later_input_and_file_checksum() {
    use std::{
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::Duration,
    };
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut decoded = Vec::new();
        DecodeSession::new(DecodePolicy::default())
            .decode_reader(stdout, |_, y, row| {
                tx.send((y, row.to_vec())).unwrap();
                decoded.extend_from_slice(row);
                Ok(())
            })
            .unwrap();
        decoded
    });
    let info = ImageInfo::new(41, 31, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(41, 5, 8 << 20));
    let mut shape = spatial();
    shape.tile_columns = 1;
    shape.tile_rows = 1;
    shape.order = TileOrder::Raster;
    let mut encoder = ChecksummedNumericFileEncoder::new(
        info.clone(),
        &Default::default(),
        shape,
        child.0.stdin.take().unwrap(),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        crate::stream_encode::ChecksummedGroupConfig::new(cfg, 1 << 20, 10, 1 << 20),
        1 << 24,
    )
    .unwrap();
    encoder.begin_tile().unwrap();
    encoder.push_group(&pixels, info.row_bytes()).unwrap();
    // The producer and both validation scopes remain open until these rows exist.
    for expected in 0..5 {
        let (y, row) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("checked first-row publication");
        assert_eq!(y, expected);
        assert_eq!(
            row,
            &pixels[y as usize * info.row_bytes()..][..info.row_bytes()]
        );
    }
    assert!(child.0.try_wait().unwrap().is_none());
    encoder.end_tile().unwrap();
    for _ in 1..encoder.layout().tile_count() {
        encoder.begin_tile().unwrap();
        let rect = encoder.next_group().unwrap();
        encoder
            .push_group(
                &pixels[rect.y as usize * info.row_bytes()..],
                info.row_bytes(),
            )
            .unwrap();
        encoder.end_tile().unwrap();
    }
    let (stdin, _, _) = encoder.finish().unwrap();
    drop(stdin);
    assert_eq!(reader.join().unwrap(), pixels);
    assert!(child.0.wait().unwrap().success());
}
