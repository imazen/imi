use super::*;
use crate::{ImageInfo, ImageRef, ImiEncoderConfig, Metadata};
use alloc::{vec, vec::Vec};

struct Counted<'a> {
    bytes: &'a [u8],
    reads: Vec<(u64, usize)>,
    batches: Vec<usize>,
}
impl ReadAt for Counted<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads.push((at, out.len()));
        SliceReader::new(self.bytes).read_exact_at(at, out)
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.batches.push(requests.len());
        for (at, bytes) in requests {
            self.read_exact_at(*at, bytes)?;
        }
        Ok(())
    }
}

impl zencodec::source::ReadAt for &mut Counted<'_> {
    type Error = Error;

    fn len(&mut self) -> Result<u64, Error> {
        panic!("the immutable length hint should be used");
    }
    fn len_hint(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        ReadAt::read_exact_at(*self, at, out)
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        ReadAt::read_many_at(*self, requests)
    }
}
fn fixture(paged: bool, checksum: bool) -> (Vec<u8>, ImageInfo, Vec<u8>) {
    let thumb = ImageInfo::new(5, 4, Channels::Gray, SampleType::U8);
    let primary = ImiEncoderConfig::lossless()
        .with_checksum(checksum)
        .encode_samples(ImageRef::packed(&[33; 20], &thumb), &Metadata::default())
        .unwrap();
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    b.add_attachment(
        AttachmentDescriptor::new("first", "application/octet-stream"),
        b"opaque first payload",
    )
    .unwrap();
    let ids: Vec<_> = ["r", "g", "b", "a"]
        .into_iter()
        .map(|n| {
            b.add_plane(PlaneDescriptor::new(n, SampleType::U8))
                .unwrap()
        })
        .collect();
    let info = ImageInfo::new(67, 61, Channels::Rgba, SampleType::U8);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 11) as u8)
        .collect();
    b.add_color_level(
        &ids,
        LevelDescriptor::new(67, 61, 7, 5),
        &pixels,
        info.row_bytes(),
        Channels::Rgba,
        ImiEncoderConfig::lossless(),
    )
    .unwrap();
    b.add_attachment(
        AttachmentDescriptor::new("last", "application/octet-stream"),
        b"opaque last payload",
    )
    .unwrap();
    b.set_image_directory(
        ImageDirectory::new(
            0,
            vec![ImageView::new(
                0,
                "original",
                ImageViewRole::Original,
                ViewBacking::Level {
                    plane: ids[0],
                    level: 0,
                },
            )],
        )
        .unwrap(),
    )
    .unwrap();
    if paged {
        b.set_record_pages(16).unwrap()
    }
    (b.finish().unwrap(), info, pixels)
}
fn crop<R: ReadAt>(r: &mut PlaneSetReader<R>, info: &ImageInfo, pixels: &[u8], rect: Rect) {
    let row = rect.width as usize * 4;
    let stride = row + 7;
    let mut out = vec![0xa7; stride * rect.height as usize];
    r.decode_view_region(0, rect, &mut out, stride).unwrap();
    for y in 0..rect.height as usize {
        let at = (rect.y as usize + y) * info.row_bytes() + rect.x as usize * 4;
        assert_eq!(&out[y * stride..][..row], &pixels[at..at + row]);
        assert!(
            out[y * stride + row..(y + 1) * stride]
                .iter()
                .all(|&v| v == 0xa7)
        );
    }
}
#[test]
fn paged_native_views_preserve_pixels_attachments_csum_and_encoded_migration() {
    for checksum in [false, true] {
        let (file, info, pixels) = fixture(true, checksum);
        let mut r =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        assert!(r.has_record_pages());
        assert!(r.dir.records.is_empty());
        assert!(r.page_cache.is_none());
        assert_eq!(r.attachment_len(AttachmentId(0)).unwrap(), 20);
        assert_eq!(
            r.read_attachment(AttachmentId(0)).unwrap(),
            b"opaque first payload"
        );
        let mut part = [0; 4];
        r.read_attachment_into_range(AttachmentId(1), 7, &mut part)
            .unwrap();
        assert_eq!(&part, b"last");
        for rect in [
            Rect::new(0, 0, 67, 61),
            Rect::new(0, 0, 1, 1),
            Rect::new(66, 60, 1, 1),
            Rect::new(5, 3, 59, 53),
        ] {
            crop(&mut r, &info, &pixels, rect)
        }
        let upgraded = crate::upgrade_image(&file, &crate::UpgradeLimits::default()).unwrap();
        let mut r =
            PlaneSetReader::open(SliceReader::new(&upgraded), PlaneSetLimits::default()).unwrap();
        assert!(r.has_record_pages());
        crop(&mut r, &info, &pixels, Rect::new(3, 2, 61, 55));
        assert_eq!(
            r.read_attachment(AttachmentId(1)).unwrap(),
            b"opaque last payload"
        );
    }
}
#[test]
fn paging_opens_only_root_and_each_selected_tile_uses_one_record_read() {
    let (file, info, pixels) = fixture(true, false);
    let mut r = PlaneSetReader::open(
        Counted {
            bytes: &file,
            reads: Vec::new(),
            batches: Vec::new(),
        },
        PlaneSetLimits::default(),
    )
    .unwrap();
    let pages = r.dir.pages.as_ref().unwrap();
    let span = pages.data_end
        ..pages.pages.last().unwrap().offset
            + wire::chunk_len(pages.pages.last().unwrap().len).unwrap();
    assert_eq!(r.source.reads.len(), 3); // Header, fixed locator, compact root.
    assert_eq!(r.source.batches, [2]); // Header + locator available concurrently.
    assert!(
        r.source
            .reads
            .iter()
            .all(|&(at, len)| at + len as u64 <= span.start || at >= span.end)
    );
    let before = r.source.reads.len();
    crop(&mut r, &info, &pixels, Rect::new(0, 0, 1, 1));
    assert_eq!(r.source.reads.len() - before, 2); // One page, one complete framed tile.
    let before = r.source.reads.len();
    crop(&mut r, &info, &pixels, Rect::new(7, 0, 1, 1));
    assert_eq!(r.source.reads.len() - before, 1); // Retained page; no header-only read.
    assert!(r.page_cache.as_ref().unwrap().1.len() <= 16);
    let (flat, _, _) = fixture(false, false);
    let mut flat = PlaneSetReader::open(
        Counted {
            bytes: &flat,
            reads: Vec::new(),
            batches: Vec::new(),
        },
        PlaneSetLimits::default(),
    )
    .unwrap();
    let before = flat.source.reads.len();
    crop(&mut flat, &info, &pixels, Rect::new(7, 0, 1, 1));
    assert_eq!(flat.source.reads.len() - before, 1); // Same whole-record improvement.
}
#[test]
fn paged_crop_merges_adjacent_tiles_and_leaves_intervening_tiles_unread() {
    let (file, info, pixels) = fixture(true, false);
    let mut r = PlaneSetReader::open_with_length(
        Counted {
            bytes: &file,
            reads: Vec::new(),
            batches: Vec::new(),
        },
        file.len() as u64,
        PlaneSetLimits::default(),
    )
    .unwrap();
    let runs = [[1, 2], [11, 12]].map(|[first, last]| {
        let a = r.record_by_id(first).unwrap();
        let b = r.record_by_id(last).unwrap();
        assert_eq!(a.offset + wire::chunk_len(a.len).unwrap(), b.offset);
        (
            a.offset,
            (b.offset - a.offset + wire::chunk_len(b.len).unwrap()) as usize,
        )
    });
    r.source.reads.clear();
    crop(&mut r, &info, &pixels, Rect::new(0, 0, 14, 10));
    assert_eq!(r.source.reads, runs);

    r.source.reads.clear();
    r.limits = r.limits.with_work_bytes(runs[0].1 as u64, u64::MAX);
    let mut out = [0xa7; 14 * 10 * 4];
    assert_eq!(
        r.decode_view_region(0, Rect::new(0, 0, 14, 10), &mut out, 14 * 4),
        Err(Error::TooLarge),
    );
    assert!(r.source.reads.is_empty());
    assert_eq!(out, [0xa7; 14 * 10 * 4]);
}

#[test]
fn zencodec_source_batches_opening_and_selected_page_reads() {
    let (file, info, pixels) = fixture(true, false);
    let mut source = Counted {
        bytes: &file,
        reads: Vec::new(),
        batches: Vec::new(),
    };
    let mut reader = PlaneSetReader::open(
        ZencodecReadAt::new(&mut source, core::convert::identity),
        PlaneSetLimits::default(),
    )
    .unwrap();
    let roi = Rect::new(0, 0, 14, 19);
    let pages = reader.view_region_pages(0, roi).unwrap();
    assert_eq!(pages.len(), 3);
    assert_eq!(reader.view_region_ranges(0, roi).unwrap().len(), 8);
    crop(&mut reader, &info, &pixels, roi);
    drop(reader);
    assert_eq!(source.batches, [2, 3]);
}
#[test]
fn page_hashes_and_bound_ids_reject_selected_damage_without_reading_unrelated_pages() {
    for damage_id in [false, true] {
        let (mut file, info, pixels) = fixture(true, false);
        let mut r =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        let at = if damage_id {
            let rec = r.record_by_id(1).unwrap();
            rec.offset + wire::chunk_len(rec.len).unwrap() - rec.len
        } else {
            let p = &r.dir.pages.as_ref().unwrap().pages[0];
            p.offset + wire::chunk_len(p.len).unwrap() - 1
        };
        drop(r);
        file[at as usize] ^= 1;
        let mut r =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        crop(&mut r, &info, &pixels, Rect::new(66, 60, 1, 1));
        let mut out = [0xa7; 4];
        assert!(matches!(
            r.decode_view_region(0, Rect::new(0, 0, 1, 1), &mut out, 4),
            Err(Error::Corrupt(_))
        ));
        assert_eq!(out, [0xa7; 4]);
    }
}
#[test]
fn root_selects_exactly_one_authority_and_bounds_page_geometry_before_payload_reads() {
    let (file, _, _) = fixture(true, false);
    let r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let root = r.dir.pages.as_ref().unwrap();
    let raw = root.encode(PlaneSetLimits::default()).unwrap();
    for end in 0..raw.len() {
        assert!(
            record_pages::RecordPages::read(
                &raw[..end],
                r.dir.primary_len,
                root.pages.last().unwrap().offset
                    + wire::chunk_len(root.pages.last().unwrap().len).unwrap(),
                PlaneSetLimits::default()
            )
            .is_err()
        )
    }
    assert!(
        PlaneSetReader::open(
            SliceReader::new(&file),
            PlaneSetLimits::default().with_records(1)
        )
        .is_err()
    );
    for n in [0, 1, 15, 17, 8192] {
        assert!(record_pages::RecordPages::check_page_records(n).is_err())
    }
    let raw = wire::encode(&r.dir, PlaneSetLimits::default(), 4).unwrap();
    let mut records = Vec::new();
    crate::parse::records::head_records(&raw, |id, payload| {
        records.push((id, payload.to_vec()));
        Ok(())
    })
    .unwrap();
    records.push((crate::registry::DIRECTORY_TILES, vec![0, 0, 0, 0]));
    records.sort_unstable_by_key(|v| v.0);
    let mut mixed = Vec::new();
    for (id, payload) in records {
        crate::parse::records::put_record(&mut mixed, id, &payload)
    }
    assert!(matches!(
        wire::decode(
            &mixed,
            PlaneSetLimits::default(),
            root.pages.last().unwrap().offset
                + wire::chunk_len(root.pages.last().unwrap().len).unwrap(),
            4
        ),
        Err(Error::Corrupt(
            "plane directory requires one record authority"
        ))
    ));
}

#[test]
fn view_range_plan_admits_selected_pages_without_reading_native_tile_payloads() {
    let (file, info, pixels) = fixture(true, false);
    let mut reader = PlaneSetReader::open(
        Counted {
            bytes: &file,
            reads: Vec::new(),
            batches: Vec::new(),
        },
        PlaneSetLimits::default(),
    )
    .unwrap();
    let rect = Rect::new(3, 4, 29, 42);
    let before = reader.source.reads.len();
    let pages = reader.view_region_pages(0, rect).unwrap();
    assert!(pages.len() > 2);
    assert_eq!(reader.source.reads.len(), before);
    let ranges = reader.view_region_ranges(0, rect).unwrap();
    let reads: Vec<_> = reader.source.reads[before..]
        .iter()
        .map(|&(at, len)| at..at + len as u64)
        .collect();
    assert_eq!(reads, pages);
    assert_eq!(ranges.len(), 50);
    assert!(ranges.iter().all(|r| r.end <= pages[0].start));
    crop(&mut reader, &info, &pixels, rect);

    reader.limits = reader.limits.with_work_bytes(1, u64::MAX);
    let before = reader.source.reads.len();
    assert!(matches!(
        reader.view_region_ranges(0, rect),
        Err(Error::TooLarge)
    ));
    assert_eq!(reader.source.reads.len(), before);
}
