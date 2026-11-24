use super::*;
use crate::{ImageInfo, ImageRef, ImiEncoderConfig, Metadata};
use std::{cell::RefCell, io, io::Cursor, rc::Rc};

// Intentionally Write-only: no Seek implementation is available to the writer.
#[derive(Clone, Default)]
struct Destination(Rc<RefCell<(Vec<u8>, usize)>>);
impl io::Write for Destination {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().1 += 1;
        Ok(())
    }
}
fn encoded(info: &ImageInfo, pixels: &[u8], stride: usize) -> Vec<u8> {
    // Match the documented native tile profile, independently of the builder.
    let config = ImiEncoderConfig::lossless().lower();
    let crate::config::Lowered::Lossless(config) = config else {
        panic!("lossless config")
    };
    let bytes =
        crate::v1::encode_frame(pixels, stride, info, &Metadata::default(), &config, None).unwrap();
    crate::v4::migrate::finish_encoded(bytes).unwrap()
}
fn primary(checksum: bool) -> Vec<u8> {
    let info = ImageInfo::new(5, 4, Channels::Gray, SampleType::U8);
    ImiEncoderConfig::lossless()
        .with_checksum(checksum)
        .encode_samples(ImageRef::packed(&[33; 20], &info), &Metadata::default())
        .unwrap()
}
fn native_directory(plane: PlaneId) -> ImageDirectory {
    ImageDirectory::new(
        0,
        vec![ImageView::new(
            0,
            "original",
            ImageViewRole::Original,
            ViewBacking::Level { plane, level: 0 },
        )],
    )
    .unwrap()
}

#[test]
fn smart_page_browser_gigapixel_fixtures() {
    let size = 32_768;
    let edge = 256;
    let info = ImageInfo::new(edge, edge, Channels::Rgb, SampleType::U8);
    let tiles: Vec<_> = (0..128)
        .map(|index| {
            let mut pixels = [17, 31, 47].repeat((edge * edge) as usize);
            for y in 0..(index % 32 + 3) {
                for x in 0..(index / 2 + 5) {
                    let at = (y * edge as usize + x) * 3;
                    pixels[at..at + 3].copy_from_slice(&[91, 110, 132]);
                }
            }
            encoded(&info, &pixels, info.row_bytes())
        })
        .collect();
    let limits = PlaneSetLimits::default();
    let mut sizes = Vec::new();
    for (name, per_page, smart) in [
        ("legacy64", 64, false),
        ("legacy1024", 1024, false),
        ("smart1024", 1024, true),
    ] {
        let mut writer = PlaneSetWriter::new(
            &primary(false),
            Vec::new(),
            Cursor::new(Vec::new()),
            limits,
            per_page,
            1 << 20,
        )
        .unwrap();
        if !smart {
            writer.use_legacy_lengths_for_test();
        }
        let planes: Vec<_> = ["r", "g", "b"]
            .map(|name| {
                writer
                    .add_plane(PlaneDescriptor::new(name, SampleType::U8))
                    .unwrap()
            })
            .to_vec();
        writer
            .declare_color_level(
                &planes,
                LevelDescriptor::new(size, size, edge, edge),
                Channels::Rgb,
                Coding::Lossless,
            )
            .unwrap();
        writer
            .set_image_directory(native_directory(planes[0]))
            .unwrap();
        for ordinal in writer.record_range(planes[0], 0).unwrap() {
            let bank = (((u64::from(ordinal) * 7_919) ^ (u64::from(ordinal) >> 4)) & 127) as usize;
            writer.write_tile(ordinal, &tiles[bank]).unwrap();
        }
        let (file, _) = writer.finish().unwrap();
        let mut reader = PlaneSetReader::open(SliceReader::new(&file), limits).unwrap();
        let root = reader.dir.pages.as_ref().unwrap();
        assert_eq!(root.packed_lengths, smart);
        assert_eq!(root.per_page, per_page);
        assert_eq!(root.count, 16_384);
        sizes.push(file.len() as u64 - root.data_end);
        let mut crop = [0; 12];
        reader
            .decode_view_region(0, Rect::new(size - 2, size - 2, 2, 2), &mut crop, 6)
            .unwrap();
        assert_eq!(crop, [17, 31, 47].repeat(4).as_slice());
        if let Some(root) = std::env::var_os("IMIJ_SMART_PAGE_FIXTURES") {
            std::fs::write(
                std::path::PathBuf::from(root).join(format!("{name}.imi")),
                file,
            )
            .unwrap();
        }
    }
    assert!(sizes[2] < sizes[1]);
}

#[test]
fn forward_native_writer_matches_buffered_bytes_and_flushes_each_record() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::U32,
        SampleType::U64,
        SampleType::I8,
        SampleType::I16,
        SampleType::I32,
        SampleType::I64,
        SampleType::F16,
        SampleType::F32,
        SampleType::F64,
    ] {
        for channels in [Channels::Gray, Channels::Rgba] {
            let info = ImageInfo::new(37, 29, channels, sample);
            let stride = info.row_bytes() + 17;
            let pixels: Vec<_> = (0..info.checked_stride_len(stride).unwrap())
                .map(|i| (i * 73 + i / 11) as u8)
                .collect();
            let p = primary(false);
            let mut buffered = PlaneSetBuilder::new(&p).unwrap();
            buffered.set_record_pages(16).unwrap();
            let destination = Destination::default();
            let mut forward = PlaneSetWriter::new(
                &p,
                destination.clone(),
                Cursor::new(b"scratch-prefix".to_vec()),
                PlaneSetLimits::default(),
                16,
                1 << 20,
            )
            .unwrap();
            let attachment = AttachmentDescriptor::new("calibration", "application/octet-stream");
            buffered
                .add_attachment(attachment.clone(), b"before")
                .unwrap();
            let first = forward.declare_attachment(attachment).unwrap();
            let ids: Vec<_> = (0..channels.count())
                .map(|c| {
                    let descriptor = PlaneDescriptor::new(format!("component-{c}"), sample);
                    let id = buffered.add_plane(descriptor.clone()).unwrap();
                    assert_eq!(forward.add_plane(descriptor).unwrap(), id);
                    id
                })
                .collect();
            let level = LevelDescriptor::new(37, 29, 7, 5);
            buffered
                .add_color_level(
                    &ids,
                    level.clone(),
                    &pixels,
                    stride,
                    channels,
                    ImiEncoderConfig::lossless(),
                )
                .unwrap();
            forward
                .declare_color_level(&ids, level, channels, Coding::Lossless)
                .unwrap();
            let attachment = AttachmentDescriptor::new("notes", "text/plain");
            buffered
                .add_attachment(attachment.clone(), b"after")
                .unwrap();
            let last = forward.declare_attachment(attachment).unwrap();
            buffered
                .set_image_directory(native_directory(ids[0]))
                .unwrap();
            forward
                .set_image_directory(native_directory(ids[0]))
                .unwrap();
            forward.write_attachment(first, b"before").unwrap();
            let range = forward.record_range(ids[0], 0).unwrap();
            for (id, (y, x)) in range.zip(
                (0..29)
                    .step_by(5)
                    .flat_map(|y| (0..37).step_by(7).map(move |x| (y, x))),
            ) {
                let tile = ImageInfo::new(7.min(37 - x), 5.min(29 - y), channels, sample);
                let at = y as usize * stride + x as usize * info.bytes_per_pixel();
                let bytes = encoded(&tile, &pixels[at..], stride);
                let before = destination.0.borrow().0.len();
                forward.write_tile(id, &bytes).unwrap();
                assert!(destination.0.borrow().0.len() > before);
                assert_eq!(
                    forward.bytes_written() as usize,
                    destination.0.borrow().0.len()
                );
            }
            forward.write_attachment(last, b"after").unwrap();
            let before_finish = destination.0.borrow().0.len();
            let flushes = destination.0.borrow().1;
            assert_eq!(forward.next_record(), None);
            let (_, scratch) = forward.finish().unwrap();
            assert!(scratch.into_inner().starts_with(b"scratch-prefix"));
            assert!(destination.0.borrow().0.len() > before_finish);
            assert_eq!(destination.0.borrow().1, flushes + 1);
            let file = buffered.finish().unwrap();
            assert_eq!(destination.0.borrow().0, file);
            let mut reader =
                PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
            let rect = Rect::new(3, 2, 33, 26);
            let row = rect.width as usize * info.bytes_per_pixel();
            let out_stride = row + 9;
            let mut out = vec![0xa7; out_stride * rect.height as usize];
            reader
                .decode_view_region(0, rect, &mut out, out_stride)
                .unwrap();
            for y in 0..rect.height as usize {
                let at = (y + 2) * stride + 3 * info.bytes_per_pixel();
                assert_eq!(&out[y * out_stride..][..row], &pixels[at..at + row]);
                assert!(
                    out[y * out_stride + row..(y + 1) * out_stride]
                        .iter()
                        .all(|&b| b == 0xa7)
                );
            }
        }
    }
}

struct BadScratch(Cursor<Vec<u8>>);
impl io::Read for BadScratch {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "scratch read",
        ))
    }
}
impl io::Write for BadScratch {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut self.0, bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl io::Seek for BadScratch {
    fn seek(&mut self, at: io::SeekFrom) -> io::Result<u64> {
        io::Seek::seek(&mut self.0, at)
    }
}
struct ChangedScratch(Cursor<Vec<u8>>);
impl io::Read for ChangedScratch {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = io::Read::read(&mut self.0, bytes)?;
        if n > 0 {
            bytes[0] ^= 1;
        }
        Ok(n)
    }
}
impl io::Write for ChangedScratch {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut self.0, bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl io::Seek for ChangedScratch {
    fn seek(&mut self, at: io::SeekFrom) -> io::Result<u64> {
        io::Seek::seek(&mut self.0, at)
    }
}

#[test]
fn forward_native_writer_rejects_bad_order_profile_limits_and_incomplete_output() {
    let p = primary(false);
    let info = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let tile = encoded(&info, &[42; 16], 4);
    for invalid in 0..4 {
        let destination = Destination::default();
        let limits = if invalid == 2 {
            PlaneSetLimits::default().with_file_bytes(p.len() as u64 + 1)
        } else {
            PlaneSetLimits::default()
        };
        let mut writer = PlaneSetWriter::new(
            &p,
            destination.clone(),
            Cursor::new(Vec::new()),
            limits,
            16,
            if invalid == 3 { 0 } else { 1000 },
        )
        .unwrap();
        let plane = writer
            .add_plane(PlaneDescriptor::new("value", SampleType::U8))
            .unwrap();
        writer
            .declare_level(plane, LevelDescriptor::new(4, 4, 4, 4), Coding::Lossless)
            .unwrap();
        let before = destination.0.borrow().0.len();
        let bad_tile = encoded(
            &ImageInfo::new(3, 4, Channels::Gray, SampleType::U8),
            &[42; 12],
            3,
        );
        assert!(
            writer
                .write_tile(
                    if invalid == 0 { 1 } else { 0 },
                    if invalid == 1 { &bad_tile } else { &tile }
                )
                .is_err()
        );
        assert_eq!(destination.0.borrow().0.len(), before);
        assert!(writer.write_tile(0, &tile).is_err()); // Failed publication poisons.
        assert!(writer.finish().is_err());
    }
    let destination = Destination::default();
    let mut writer = PlaneSetWriter::new(
        &p,
        destination.clone(),
        Cursor::new(Vec::new()),
        PlaneSetLimits::default(),
        16,
        1000,
    )
    .unwrap();
    let plane = writer
        .add_plane(PlaneDescriptor::new("value", SampleType::U8))
        .unwrap();
    writer
        .declare_level(plane, LevelDescriptor::new(8, 4, 4, 4), Coding::Lossless)
        .unwrap();
    writer.write_tile(0, &tile).unwrap();
    assert!(
        writer
            .add_plane(PlaneDescriptor::new("late", SampleType::U8))
            .is_err()
    );
    let before = destination.0.borrow().0.len();
    assert!(writer.finish().is_err());
    assert_eq!(destination.0.borrow().0.len(), before);
}

#[test]
fn forward_native_writer_preserves_read_errors_and_refuses_outer_csum_before_output() {
    let destination = Destination::default();
    assert!(
        PlaneSetWriter::new(
            &primary(true),
            destination.clone(),
            Cursor::new(Vec::new()),
            PlaneSetLimits::default(),
            16,
            1000
        )
        .is_err()
    );
    assert!(destination.0.borrow().0.is_empty());
    let p = primary(false);
    let mut writer = PlaneSetWriter::new(
        &p,
        destination.clone(),
        BadScratch(Cursor::new(Vec::new())),
        PlaneSetLimits::default(),
        16,
        1000,
    )
    .unwrap();
    let attachment = writer
        .declare_attachment(AttachmentDescriptor::new("one", "text/plain"))
        .unwrap();
    writer.write_attachment(attachment, b"opaque").unwrap();
    let before = destination.0.borrow().0.len();
    assert!(matches!(
        writer.finish(),
        Err(crate::stream_encode::StreamEncodeError::Read(_))
    ));
    assert_eq!(destination.0.borrow().0.len(), before); // No pages, root or locator.
    let mut writer = PlaneSetWriter::new(
        &p,
        destination.clone(),
        ChangedScratch(Cursor::new(Vec::new())),
        PlaneSetLimits::default(),
        16,
        1000,
    )
    .unwrap();
    let attachment = writer
        .declare_attachment(AttachmentDescriptor::new("changed", "text/plain"))
        .unwrap();
    writer.write_attachment(attachment, b"opaque").unwrap();
    let before = destination.0.borrow().0.len();
    assert!(matches!(
        writer.finish(),
        Err(crate::stream_encode::StreamEncodeError::Codec(
            Error::InvalidInput("native plane directory scratch changed")
        ))
    ));
    assert_eq!(destination.0.borrow().0.len(), before);
}

struct Tracked<'a> {
    bytes: &'a [u8],
    reads: Vec<(u64, usize)>,
}
impl ReadAt for Tracked<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads.push((at, out.len()));
        SliceReader::new(self.bytes).read_exact_at(at, out)
    }
}

#[test]
fn forward_16k_and_gigapixel_views_need_no_whole_raster_or_record_array() {
    for (size, tile_size) in [(16384, 256), (32768, 512)] {
        let p = primary(false);
        let limits = PlaneSetLimits::default();
        let mut writer = PlaneSetWriter::new(
            &p,
            Vec::new(),
            Cursor::new(Vec::new()),
            limits,
            256,
            1 << 20,
        )
        .unwrap();
        let plane = writer
            .add_plane(PlaneDescriptor::new("value", SampleType::U8))
            .unwrap();
        writer
            .declare_level(
                plane,
                LevelDescriptor::new(size, size, tile_size, tile_size),
                Coding::Lossless,
            )
            .unwrap();
        writer.set_image_directory(native_directory(plane)).unwrap();
        let info = ImageInfo::new(tile_size, tile_size, Channels::Gray, SampleType::U8);
        let tile = encoded(
            &info,
            &vec![42; info.checked_buffer_len().unwrap()],
            info.row_bytes(),
        );
        for id in writer.record_range(plane, 0).unwrap() {
            writer.write_tile(id, &tile).unwrap()
        }
        let (file, scratch) = writer.finish().unwrap();
        assert!(scratch.into_inner().len() <= 4096 * 10);
        let mut reader = PlaneSetReader::open(
            Tracked {
                bytes: &file,
                reads: Vec::new(),
            },
            limits,
        )
        .unwrap();
        assert_eq!(reader.source.reads.len(), 3);
        let opening_bytes: usize = reader.source.reads.iter().map(|r| r.1).sum();
        assert!(reader.dir.records.is_empty());
        assert_eq!(reader.dir.pages.as_ref().unwrap().pages.len(), 16);
        let stride = 7;
        let mut out = [0xa7; 14];
        reader
            .decode_view_region(0, Rect::new(size - 2, size - 2, 2, 2), &mut out, stride)
            .unwrap();
        assert_eq!(
            out,
            [
                42, 42, 0xa7, 0xa7, 0xa7, 0xa7, 0xa7, 42, 42, 0xa7, 0xa7, 0xa7, 0xa7, 0xa7
            ]
        );
        assert_eq!(reader.page_cache.as_ref().unwrap().1.len(), 256);
        assert_eq!(reader.source.reads.len(), 5);
        let cold_bytes: usize = reader.source.reads[3..].iter().map(|r| r.1).sum();
        reader
            .decode_view_region(0, Rect::new(size - 2, size - 2, 2, 2), &mut out, stride)
            .unwrap();
        assert_eq!(reader.source.reads.len(), 6);
        let pages = reader.dir.pages.as_ref().unwrap();
        let raw_index = wire::encode(&reader.dir, limits, 4).unwrap().len();
        let page_bytes: u64 = pages
            .pages
            .iter()
            .map(|p| wire::chunk_len(p.len).unwrap())
            .sum();
        let receipt = serde_json::json!({
            "geometry": [size, size], "tile": [tile_size, tile_size],
            "records": pages.count, "pages": pages.pages.len(),
            "file_bytes": file.len(), "expanded_root_bytes": raw_index,
            "packed_root_chunk_bytes": reader.source.reads[2].1,
            "all_page_chunk_bytes": page_bytes,
            "opening_reads": 3, "opening_bytes": opening_bytes,
            "cold_roi_reads": 2, "cold_roi_bytes": cold_bytes,
            "warm_page_roi_reads": 1, "warm_page_roi_bytes": reader.source.reads[5].1,
            "witness": "synthetic constant native pixels; source reads, not HTTP or timing"
        });
        if let Ok(path) = std::env::var("IMI_DIRECTORY_RECEIPT") {
            use io::Write;
            let mut output = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(output, "{receipt}").unwrap();
        }
    }
}

/// Retains the real small writes and treats one reused zero-filled source
/// buffer as a hole. The destination deliberately implements only Write.
struct SparseDestination {
    position: u64,
    zero_source: usize,
    spans: Vec<(u64, Vec<u8>)>,
    flushes: usize,
}

impl io::Write for SparseDestination {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.as_ptr() as usize != self.zero_source {
            self.spans.push((self.position, bytes.to_vec()));
        }
        self.position += bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

struct SparseSource<'a> {
    file: &'a SparseDestination,
    reads: Vec<(u64, usize)>,
}

impl ReadAt for SparseSource<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.file.position)
    }

    fn read_exact_at(&mut self, at: u64, output: &mut [u8]) -> Result<(), Error> {
        let end = at.checked_add(output.len() as u64).ok_or(Error::TooLarge)?;
        assert!(end <= self.file.position);
        self.reads.push((at, output.len()));
        output.fill(0);
        for (start, data) in &self.file.spans {
            let left = (*start).max(at);
            let right = (*start + data.len() as u64).min(end);
            if left < right {
                output[(left - at) as usize..(right - at) as usize]
                    .copy_from_slice(&data[(left - start) as usize..(right - start) as usize]);
            }
        }
        Ok(())
    }
}

#[test]
fn default_gigapixel_crop_reads_real_forward_records_beyond_eight_gib() {
    let zeros = vec![0; 128 << 20];
    let destination = SparseDestination {
        position: 0,
        zero_source: zeros.as_ptr() as usize,
        spans: Vec::new(),
        flushes: 0,
    };
    let primary = primary(false);
    let limits = PlaneSetLimits::default();
    let mut writer = PlaneSetWriter::new(
        &primary,
        destination,
        Cursor::new(Vec::new()),
        limits,
        256,
        1 << 20,
    )
    .unwrap();
    let attachments: Vec<_> = (0..65)
        .map(|i| {
            writer
                .declare_attachment(AttachmentDescriptor::new(
                    format!("padding-{i}"),
                    "application/octet-stream",
                ))
                .unwrap()
        })
        .collect();
    let plane = writer
        .add_plane(PlaneDescriptor::new("value", SampleType::U8))
        .unwrap();
    let size = 32_768;
    let tile_size = 512;
    writer
        .declare_level(
            plane,
            LevelDescriptor::new(size, size, tile_size, tile_size),
            Coding::Lossless,
        )
        .unwrap();
    writer.set_image_directory(native_directory(plane)).unwrap();
    for &attachment in &attachments {
        writer.write_attachment(attachment, &zeros).unwrap();
    }
    let first_tile = writer.bytes_written();
    assert!(first_tile > 8 << 30);
    let info = ImageInfo::new(tile_size, tile_size, Channels::Gray, SampleType::U8);
    let tile = encoded(
        &info,
        &vec![37; info.checked_buffer_len().unwrap()],
        info.row_bytes(),
    );
    for id in writer.record_range(plane, 0).unwrap() {
        writer.write_tile(id, &tile).unwrap();
    }
    let before_directory = writer.bytes_written();
    let (destination, scratch) = writer.finish().unwrap();
    assert!(destination.position > before_directory);
    assert!(destination.flushes > 4096 + attachments.len());
    let source = || SparseSource {
        file: &destination,
        reads: Vec::new(),
    };

    assert!(matches!(
        PlaneSetReader::open(source(), limits.with_file_bytes(8 << 30)),
        Err(Error::TooLarge),
    ));
    assert!(matches!(
        PlaneSetReader::open(source(), limits.with_plane_pixels(1_000_000_000)),
        Err(Error::TooLarge),
    ));
    let mut reader = PlaneSetReader::open(source(), limits).unwrap();
    let opening = reader.source.reads.len();
    let mut output = [0xa7; 8];
    reader
        .decode_view_region(0, Rect::new(size - 2, size - 2, 2, 2), &mut output, 4)
        .unwrap();
    assert_eq!(output, [37, 37, 0xa7, 0xa7, 37, 37, 0xa7, 0xa7]);
    let reads = &reader.source.reads[opening..];
    assert_eq!(reads.len(), 2);
    assert!(reads.iter().all(|(offset, _)| *offset >= first_tile));
    let mut opaque = [0xa7; 5];
    reader
        .read_attachment_into_range(attachments[64], zeros.len() as u64 - 5, &mut opaque)
        .unwrap();
    assert_eq!(opaque, [0; 5]);

    println!(
        "file_bytes={} first_tile={} open_reads={} open_bytes={} cold_crop_reads={} cold_crop_bytes={} scratch_bytes={} retained_bytes={}",
        destination.position,
        first_tile,
        opening,
        reader.source.reads[..opening]
            .iter()
            .map(|(_, bytes)| bytes)
            .sum::<usize>(),
        2,
        reader.source.reads[opening..opening + 2]
            .iter()
            .map(|(_, bytes)| bytes)
            .sum::<usize>(),
        scratch.into_inner().len(),
        destination
            .spans
            .iter()
            .map(|(_, bytes)| bytes.len())
            .sum::<usize>(),
    );
}

#[test]
fn native_tile_pixel_checksum_failure_keeps_that_tiles_pixels_private() {
    let limits = PlaneSetLimits::default();
    let info = ImageInfo::new(2, 2, Channels::Gray, SampleType::U8);
    let cfg = ImiEncoderConfig::lossless().with_checksum(false);
    let seal = |pixels: &[u8; 4]| {
        let bytes = cfg
            .encode_samples(ImageRef::packed(pixels, &info), &Metadata::default())
            .unwrap();
        assert!(!bytes.windows(4).any(|tag| tag == b"cSUM"));
        crate::add_pixel_checksum(bytes).unwrap()
    };
    let first = seal(&[11, 12, 13, 14]);
    let mut second = seal(&[21, 22, 23, 24]);
    let at = second.windows(4).position(|tag| tag == b"pSUM").unwrap();
    let mut payload = crate::ByteReader::new(&second[at..]);
    let (_, len) = crate::parse::chunks::read_head(&mut payload).unwrap();
    let digest_end = at + payload.pos + len.unwrap() as usize;
    second[digest_end - 1] ^= 1;

    let mut writer = PlaneSetWriter::new(
        &primary(false),
        Vec::new(),
        Cursor::new(Vec::new()),
        limits,
        64,
        1 << 20,
    )
    .unwrap();
    let plane = writer
        .add_plane(PlaneDescriptor::new("gray", SampleType::U8))
        .unwrap();
    writer
        .declare_level(plane, LevelDescriptor::new(4, 2, 2, 2), Coding::Lossless)
        .unwrap();
    writer.set_image_directory(native_directory(plane)).unwrap();
    let mut records = writer.record_range(plane, 0).unwrap();
    writer.write_tile(records.next().unwrap(), &first).unwrap();
    writer.write_tile(records.next().unwrap(), &second).unwrap();
    let (bytes, _) = writer.finish().unwrap();
    let mut reader = PlaneSetReader::open(SliceReader::new(&bytes), limits).unwrap();
    let mut output = [0xa7; 12];
    let error = reader
        .decode_view_region(0, Rect::new(0, 0, 4, 2), &mut output, 6)
        .unwrap_err();
    assert_eq!(error, crate::pixelsum::MISMATCH);
    assert_eq!(
        output,
        [
            11, 12, 0xa7, 0xa7, 0xa7, 0xa7, 13, 14, 0xa7, 0xa7, 0xa7, 0xa7
        ]
    );
}
