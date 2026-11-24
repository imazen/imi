mod common;
use imij::planes::SliceReader;
use imij::validation::{ValidationLimits, ValidationReport, validate_v3};
use imij::{Channels, Error, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::io::Cursor;

fn validate(bytes: &[u8]) -> Result<ValidationReport, Error> {
    validate_v3(
        &mut SliceReader::new(bytes),
        &mut Cursor::new(Vec::new()),
        &ValidationLimits::default(),
    )
}
fn image() -> Vec<u8> {
    let info = ImageInfo::new(32, 35, Channels::Gray, SampleType::U8);
    let pixels: Vec<u8> = (0..32 * 35).map(|i| (i * 31 + i / 17) as u8).collect();
    ImiEncoderConfig::quick()
        .with_checksum(false)
        .encode_legacy_samples(ImageRef::new(&pixels, 32, &info), &Metadata::default())
        .unwrap()
}
fn without_file_checksum(bytes: &[u8]) -> Vec<u8> {
    let head = common::header_without(bytes, &[2, 4]);
    let (_, mut chunks) = common::parts(bytes);
    chunks.retain(|(tag, _)| tag != b"cSUM");
    common::join(&head, &chunks, false)
}

#[test]
fn strict_validation_of_migrated_frozen_images() {
    for old in [
        include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/25-seed-csum-table-last.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/27-seed-csum-lossy.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/28-seed-csum-animation.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/29-seed-csum-previews.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/30-seed-csum-mixed-pages.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/33-seed-v2-fram-records.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/34-seed-v2-plane-directory.imi").as_slice(),
    ] {
        let upgraded = imij::upgrade_image(old, &imij::UpgradeLimits::default()).unwrap();
        let report = validate(&upgraded).unwrap();
        assert!(report.images > 0);
        assert!(report.decoded_pixels > 0);
        assert!(validate(old).is_err());
    }
}
#[test]
fn wrong_pixel_digest_fails_even_without_a_file_checksum() {
    let bytes = without_file_checksum(&imij::add_pixel_checksum(image()).unwrap());
    let report = validate(&bytes).unwrap();
    assert_eq!(report.pixel_checksums, 1);
    assert_eq!(report.file_checksums, 0);
    let (head, mut chunks) = common::parts(&bytes);
    let psum = &mut chunks.iter_mut().find(|(tag, _)| tag == b"pSUM").unwrap().1;
    *psum.last_mut().unwrap() ^= 1;
    assert_eq!(
        validate(&common::join(&head, &chunks, false)),
        Err(Error::Corrupt("pixel checksum mismatch"))
    );
}
#[test]
fn preview_entropy_must_decode_and_legacy_nested_images_fail() {
    let bytes = without_file_checksum(&image());
    let (head, mut chunks) = common::parts(&bytes);
    chunks.insert(0, (*b"pRVW", bytes.clone()));
    assert_eq!(
        validate(&common::join(&head, &chunks, false))
            .unwrap()
            .images,
        2
    );
    let preview = &mut chunks[0].1;
    *preview.last_mut().unwrap() ^= 0x80;
    // A valid header is not a validation result; malformed compressed data fails.
    assert!(imij::header(preview).is_ok());
    // Use a definite truncated record rather than assuming a changed pixel is corrupt.
    preview.pop();
    assert!(validate(&common::join(&head, &chunks, false)).is_err());
    chunks[0].1 = include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi").to_vec();
    assert!(matches!(
        validate(&common::join(&head, &chunks, false)),
        Err(Error::Unsupported(_))
    ));
}
#[test]
fn unified_references_and_legacy_rectangles_are_decoded() {
    let info = ImageInfo::new(67, 75, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * 75)
        .map(|i| ((i * 71) ^ (i / 17)) as u8)
        .collect();
    for lossy in [false, true] {
        let config = if lossy {
            ImiEncoderConfig::lossy(90.0)
        } else {
            ImiEncoderConfig::default()
        };
        let mut seq = imij::SequenceEncoder::new(
            imij::SequenceKind::Pages,
            imij::FrameConfig::new(config.clone()),
        );
        let first = seq
            .push(imij::SequenceItem::new(&pixels, info.row_bytes(), &info))
            .unwrap();
        let mut changed = first.clone();
        changed[info.row_bytes() * 34 + 5] ^= 13;
        seq.push(
            imij::SequenceItem::new(&changed, info.row_bytes(), &info).with_reference(
                imij::ImageReference {
                    index: 0,
                    pixels: &first,
                    stride: info.row_bytes(),
                },
            ),
        )
        .unwrap();
        let bytes = seq.finish().unwrap();
        let report = validate(&bytes).unwrap();
        assert_eq!(report.images, 2);
        assert_eq!(report.reference_checksums, 1);
        assert_eq!(
            report.scratch_bytes,
            (info.row_bytes() * info.height as usize) as u64
        );
        let mut legacy = imij::FrameEncoder::animation(
            info.clone(),
            Metadata::default(),
            imij::FrameConfig::new(config),
        );
        for px in [&pixels, &changed, &changed] {
            legacy
                .push_frame(px, info.row_bytes(), imij::FrameDuration::from_millis(20))
                .unwrap();
        }
        let bytes = imij::add_pixel_checksum(legacy.finish().unwrap()).unwrap();
        let report = validate(&bytes).unwrap();
        assert!(report.pixel_checksums >= 3);
    }
}
#[test]
fn gain_maps_and_every_plane_tile_are_decoded() {
    use imij::planes::*;
    let primary = image();
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let metadata = Metadata {
        gain_map: Some(imij::GainMap {
            params: imij::GainMapParams::default(),
            alt_cicp: None,
            alt_icc: None,
            image: primary.clone(),
        }),
        ..Default::default()
    };
    let gain = ImiEncoderConfig::default()
        .encode_samples(ImageRef::new(&[7], 1, &info), &metadata)
        .unwrap();
    let report = validate(&gain).unwrap();
    assert_eq!(report.images, 2);
    assert_eq!(report.scratch_bytes, 0);
    let mut builder = PlaneSetBuilder::new(&primary).unwrap();
    let plane = builder
        .add_plane(PlaneDescriptor::new("depth", SampleType::U8))
        .unwrap();
    builder
        .add_level(
            plane,
            LevelDescriptor::new(2, 2, 1, 1),
            &[1, 2, 3, 4],
            2,
            ImiEncoderConfig::default().with_effort(0.0),
        )
        .unwrap();
    builder
        .add_attachment(AttachmentDescriptor::new("note", "text/plain"), b"opaque")
        .unwrap();
    let bytes = builder.finish().unwrap();
    let report = validate(&bytes).unwrap();
    assert_eq!(report.images, 5);
    assert_eq!(report.scratch_bytes, 0);
}
#[test]
fn limits_fail_without_partial_success_and_scratch_prefix_is_preserved() {
    let bytes = image();
    let mut limits = ValidationLimits::default();
    limits.max_scratch_bytes = 0;
    let mut scratch = Cursor::new(vec![9, 8, 7]);
    let report = validate_v3(&mut SliceReader::new(&bytes), &mut scratch, &limits).unwrap();
    assert_eq!(report.scratch_bytes, 0);
    assert_eq!(scratch.into_inner(), [9, 8, 7]);
    // Reference-coded frames need retained rows; reject before any scratch write.
    let old = include_bytes!("../../fuzz/seeds/28-seed-csum-animation.imi");
    let animation = imij::upgrade_image(old, &imij::UpgradeLimits::default()).unwrap();
    let mut scratch = Cursor::new(vec![9, 8, 7]);
    assert_eq!(
        validate_v3(&mut SliceReader::new(&animation), &mut scratch, &limits),
        Err(Error::TooLarge)
    );
    assert_eq!(scratch.into_inner(), [9, 8, 7]);
    limits = ValidationLimits::default();
    limits.max_decoded_pixels = 32 * 35 - 1;
    assert_eq!(
        validate_v3(
            &mut SliceReader::new(&bytes),
            &mut Cursor::new(Vec::new()),
            &limits
        ),
        Err(Error::TooLarge)
    );
}

#[test]
fn optional_chunks_over_four_gibibytes_use_wide_offsets() {
    struct Sparse {
        prefix: Vec<u8>,
        gap: u64,
        suffix: Vec<u8>,
        max_read: usize,
    }
    impl imij::planes::ReadAt for Sparse {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.prefix.len() as u64 + self.gap + self.suffix.len() as u64)
        }
        fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
            self.max_read = self.max_read.max(out.len());
            assert!(out.len() <= 65536, "whole-file read attempted");
            for (i, b) in out.iter_mut().enumerate() {
                let p = at + i as u64;
                *b = if p < self.prefix.len() as u64 {
                    self.prefix[p as usize]
                } else if p < self.prefix.len() as u64 + self.gap {
                    0
                } else {
                    self.suffix[(p - self.prefix.len() as u64 - self.gap) as usize]
                };
            }
            Ok(())
        }
    }
    let bytes = without_file_checksum(&image());
    let mut prefix = bytes.clone();
    let gap = (1u64 << 32) + 7;
    prefix.extend_from_slice(b"zZZz");
    common::put_varint(&mut prefix, gap + 1);
    let mut suffix = Vec::new();
    common::put_chunk(&mut suffix, b"pRVW", &bytes);
    let mut source = Sparse {
        prefix,
        gap,
        suffix,
        max_read: 0,
    };
    let report = validate_v3(
        &mut source,
        &mut Cursor::new(Vec::new()),
        &ValidationLimits::default(),
    )
    .unwrap();
    assert_eq!(report.images, 2);
    assert_eq!(report.opaque_chunks, 1);
    assert_eq!(report.file_checksums, 0);
    assert!(source.max_read <= 65536);
}

#[test]
fn strict_checks_known_records_and_rejects_unknown_checksum_algorithms() {
    let bytes = without_file_checksum(&imij::add_pixel_checksum(image()).unwrap());
    let (head, chunks) = common::parts(&bytes);
    let mut changed = chunks.clone();
    let psum = &mut changed
        .iter_mut()
        .find(|(tag, _)| tag == b"pSUM")
        .unwrap()
        .1;
    psum[1] = 255;
    assert!(matches!(
        validate(&common::join(&head, &changed, false)),
        Err(Error::Unsupported(_))
    ));
    for tag in [*b"kVAL", *b"kVMD"] {
        let mut changed = chunks.clone();
        changed.push((tag, vec![255]));
        assert!(matches!(
            validate(&common::join(&head, &changed, false)),
            Err(Error::Unsupported(_))
        ));
    }
    let mut changed = chunks.clone();
    changed.push((*b"cALB", vec![255]));
    assert!(matches!(
        validate(&common::join(&head, &changed, false)),
        Err(Error::Unsupported(_))
    ));
    let mut changed = chunks;
    changed.push((*b"abca", vec![]));
    assert!(matches!(
        validate(&common::join(&head, &changed, false)),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn corrupt_reference_digest_is_not_replaced_by_the_available_raster() {
    let info = ImageInfo::new(67, 75, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * 75)
        .map(|i| ((i * 71) ^ (i / 17)) as u8)
        .collect();
    let mut seq = imij::SequenceEncoder::new(
        imij::SequenceKind::Pages,
        imij::FrameConfig::new(ImiEncoderConfig::default()),
    );
    let first = seq
        .push(imij::SequenceItem::new(&pixels, info.row_bytes(), &info))
        .unwrap();
    seq.push(
        imij::SequenceItem::new(&first, info.row_bytes(), &info).with_reference(
            imij::ImageReference {
                index: 0,
                pixels: &first,
                stride: info.row_bytes(),
            },
        ),
    )
    .unwrap();
    let bytes = without_file_checksum(&seq.finish().unwrap());
    assert_eq!(validate(&bytes).unwrap().reference_checksums, 1);
    let (head, mut chunks) = common::parts(&bytes);
    let frame = &mut chunks.iter_mut().find(|(t, _)| t == b"fRAM").unwrap().1;
    let mut pos = 0;
    for _ in 0..5 {
        pos += common::varint(&frame[pos..]).1;
    }
    pos += common::varint(&frame[pos..]).1; // record list length
    assert_eq!(common::varint(&frame[pos..]).0, 111);
    pos += common::varint(&frame[pos..]).1; // record id
    pos += common::varint(&frame[pos..]).1; // record length
    pos += common::varint(&frame[pos..]).1; // reference ordinal
    frame[pos + 1] ^= 1; // algorithm remains valid, digest differs
    assert_eq!(
        validate(&common::join(&head, &chunks, false)),
        Err(Error::Corrupt("reference checksum mismatch"))
    );
}

#[test]
fn legal_unit_table_fallbacks_are_not_mistaken_for_incomplete_validation() {
    use common::unit_table::{Form, with_form};
    let (info, pixels) =
        imij::decode(include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi")).unwrap();
    let base = ImiEncoderConfig::lossless()
        .with_checksum(false)
        .tune_lossless(|t| t.stripe_rows = Some(2))
        .encode_legacy_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
        .unwrap();
    for form in [
        Form::Hint,
        Form::Trailing,
        Form::Reserved(8),
        Form::Empty(8),
        Form::Tail(8),
    ] {
        let bytes = without_file_checksum(&with_form(&base, form));
        validate(&bytes).unwrap();
        let (head, mut chunks) = common::parts(&bytes);
        chunks.push((*b"zZZz", vec![7]));
        validate(&common::join(&head, &chunks, false)).unwrap();
        chunks.retain(|(tag, _)| tag != b"uIDX");
        validate(&common::join(&head, &chunks, false)).unwrap();
    }
}

#[test]
fn streaming_pixel_checksums_do_not_read_or_write_scratch() {
    struct NoRasterIo;
    impl std::io::Read for NoRasterIo {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("raster read")
        }
    }
    impl std::io::Write for NoRasterIo {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            panic!("raster write")
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl std::io::Seek for NoRasterIo {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            assert_eq!(pos, std::io::SeekFrom::End(0));
            Ok(0)
        }
    }
    for algorithm in [imij::HashAlgorithm::Xxh3_64, imij::HashAlgorithm::Xxh3_128] {
        let bytes = imij::add_pixel_checksum_with(image(), algorithm).unwrap();
        let mut limits = ValidationLimits::default();
        limits.max_scratch_bytes = 0;
        let report = validate_v3(&mut SliceReader::new(&bytes), &mut NoRasterIo, &limits).unwrap();
        assert_eq!(report.pixel_checksums, 1);
        assert_eq!(report.scratch_bytes, 0);
        let (head, mut chunks) = common::parts(&bytes);
        let psum = &mut chunks.iter_mut().find(|(tag, _)| tag == b"pSUM").unwrap().1;
        *psum.last_mut().unwrap() ^= 1;
        let corrupt = common::join(&head, &chunks, false);
        assert_eq!(
            validate_v3(&mut SliceReader::new(&corrupt), &mut NoRasterIo, &limits),
            Err(Error::Corrupt("pixel checksum mismatch"))
        );
    }
}

#[test]
fn independent_pages_verify_outer_ordinals_without_scratch() {
    let info = ImageInfo::new(32, 35, Channels::Gray, SampleType::U8);
    let mut pages = imij::FrameEncoder::pages(
        info.clone(),
        Metadata::default(),
        imij::FrameConfig::new(ImiEncoderConfig::quick()),
    );
    for value in [7, 91, 138] {
        pages.push_page(&vec![value; 32 * 35], 32, &info).unwrap();
    }
    let bytes = imij::add_pixel_checksum(pages.finish().unwrap()).unwrap();
    let mut limits = ValidationLimits::default();
    limits.max_scratch_bytes = 0;
    let report = validate_v3(
        &mut SliceReader::new(&bytes),
        &mut Cursor::new(Vec::new()),
        &limits,
    )
    .unwrap();
    assert_eq!(report.images, 3);
    assert_eq!(report.pixel_checksums, 3);
    assert_eq!(report.scratch_bytes, 0);
    let (head, mut chunks) = common::parts(&without_file_checksum(&bytes));
    *chunks
        .iter_mut()
        .find(|(tag, _)| tag == b"pSUM")
        .unwrap()
        .1
        .last_mut()
        .unwrap() ^= 1;
    assert_eq!(
        validate(&common::join(&head, &chunks, false)),
        Err(Error::Corrupt("pixel checksum mismatch"))
    );
}

#[test]
fn only_explicitly_referenced_pages_are_retained() {
    let info = ImageInfo::new(37, 41, Channels::Gray, SampleType::U8);
    let bytes = info.checked_buffer_len().unwrap();
    let pixels: Vec<u8> = (0..bytes).map(|i| (i * 31 + i / 13) as u8).collect();
    let mut seq = imij::SequenceEncoder::new(
        imij::SequenceKind::Pages,
        imij::FrameConfig::new(ImiEncoderConfig::default()),
    );
    // The primary and page 2 are never referenced. Page 1 is used twice,
    // across the intervening independent page, and the final pages are unused.
    seq.push(imij::SequenceItem::new(&vec![17; bytes], 37, &info))
        .unwrap();
    let reference = seq
        .push(imij::SequenceItem::new(&pixels, 37, &info))
        .unwrap();
    seq.push(imij::SequenceItem::new(&vec![91; bytes], 37, &info))
        .unwrap();
    for at in [13, 97] {
        let mut changed = reference.clone();
        changed[at] ^= 7;
        seq.push(imij::SequenceItem::new(&changed, 37, &info).with_reference(
            imij::ImageReference {
                index: 1,
                pixels: &reference,
                stride: 37,
            },
        ))
        .unwrap();
    }
    let encoded = imij::add_pixel_checksum(seq.finish().unwrap()).unwrap();
    let mut limits = ValidationLimits::default();
    limits.max_scratch_bytes = bytes as u64;
    let mut scratch = Cursor::new(vec![4, 3, 2, 1]);
    let report = validate_v3(&mut SliceReader::new(&encoded), &mut scratch, &limits).unwrap();
    assert_eq!(report.images, 5);
    assert_eq!(report.reference_checksums, 2);
    assert_eq!(report.pixel_checksums, 5);
    assert_eq!(report.scratch_bytes, bytes as u64);
    assert_eq!(&scratch.get_ref()[..4], &[4, 3, 2, 1]);
    assert_eq!(&scratch.get_ref()[4..], &reference);
    limits.max_scratch_bytes -= 1;
    assert_eq!(
        validate_v3(
            &mut SliceReader::new(&encoded),
            &mut Cursor::new(Vec::new()),
            &limits
        ),
        Err(Error::TooLarge)
    );
}
