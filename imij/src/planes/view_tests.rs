use super::*;
use crate::{ImageInfo, ImageRef, ImiEncoderConfig, Metadata};
use alloc::{vec, vec::Vec};

fn pixels(info: &ImageInfo, stride: usize) -> Vec<u8> {
    (0..info.checked_stride_len(stride).unwrap())
        .map(|i| (i * 73 + i / 11) as u8)
        .collect()
}
fn check_crop<R: ReadAt>(
    reader: &mut PlaneSetReader<R>,
    id: u64,
    info: &ImageInfo,
    pixels: &[u8],
    source_stride: usize,
) {
    assert_eq!(reader.view_info(id).unwrap(), *info);
    let rect = Rect::new(1, 2, info.width - 2, info.height - 3);
    let row = rect.width as usize * info.bytes_per_pixel();
    let stride = row + 13;
    let mut out = vec![0xa7; stride * rect.height as usize];
    reader
        .decode_view_region(id, rect, &mut out, stride)
        .unwrap();
    for y in 0..rect.height as usize {
        let at = (rect.y as usize + y) * source_stride + rect.x as usize * info.bytes_per_pixel();
        assert_eq!(&out[y * stride..][..row], &pixels[at..at + row]);
        assert!(
            out[y * stride + row..(y + 1) * stride]
                .iter()
                .all(|&b| b == 0xa7)
        );
    }
}

#[test]
fn native_image_views_reuse_primary_and_packed_levels_with_explicit_registration() {
    for checksummed in [false, true] {
        let thumb = ImageInfo::new(6, 5, Channels::Rgba, SampleType::U8);
        let tp = pixels(&thumb, thumb.row_bytes());
        let metadata = Metadata {
            orientation: 6,
            xmp: Some(b"native views".to_vec()),
            ..Default::default()
        };
        let primary = ImiEncoderConfig::lossless()
            .with_checksum(checksummed)
            .encode_samples(ImageRef::packed(&tp, &thumb), &metadata)
            .unwrap();
        let mut builder = PlaneSetBuilder::new(&primary).unwrap();
        let ids: Vec<_> = ["red", "green", "blue", "alpha"]
            .into_iter()
            .map(|n| {
                builder
                    .add_plane(PlaneDescriptor::new(n, SampleType::U8))
                    .unwrap()
            })
            .collect();
        let original = ImageInfo::new(37, 29, Channels::Rgba, SampleType::U8);
        let stride = original.row_bytes() + 17;
        let op = pixels(&original, stride);
        let levels = builder
            .add_color_level(
                &ids,
                LevelDescriptor::new(37, 29, 11, 13),
                &op,
                stride,
                Channels::Rgba,
                ImiEncoderConfig::lossless(),
            )
            .unwrap();
        let reduced = ImageInfo::new(19, 11, Channels::Rgba, SampleType::U8);
        let rp = pixels(&reduced, reduced.row_bytes());
        let reduced_levels = builder
            .add_color_level(
                &ids,
                LevelDescriptor::new(19, 11, 7, 5),
                &rp,
                reduced.row_bytes(),
                Channels::Rgba,
                ImiEncoderConfig::lossless(),
            )
            .unwrap();
        let before = builder.dir.records.len();
        let images = ImageDirectory::new(
            u64::MAX,
            vec![
                ImageView::new(
                    u64::MAX,
                    "original",
                    ImageViewRole::Original,
                    ViewBacking::Level {
                        plane: ids[2],
                        level: levels[2],
                    },
                ),
                ImageView::new(0, "thumbnail", ImageViewRole::Preview, ViewBacking::Primary)
                    .with_registration(ViewRegistration::resized(37, 29, 6, 5).unwrap()),
                ImageView::new(
                    17,
                    "reduced",
                    ImageViewRole::Reduced,
                    ViewBacking::Level {
                        plane: ids[0],
                        level: reduced_levels[0],
                    },
                )
                .with_registration(ViewRegistration::resized(37, 29, 19, 11).unwrap()),
            ],
        )
        .unwrap();
        builder.set_image_directory(images.clone()).unwrap();
        assert_eq!(builder.dir.records.len(), before); // No duplicate original/tiles.
        let file = builder.finish().unwrap();
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        assert_eq!(reader.image_directory(), Some(&images));
        assert_eq!(reader.primary_header().unwrap().orientation, 6);
        check_crop(&mut reader, 0, &thumb, &tp, thumb.row_bytes());
        check_crop(&mut reader, u64::MAX, &original, &op, stride);
        check_crop(&mut reader, 17, &reduced, &rp, reduced.row_bytes());
        let upgraded = crate::upgrade_image(&file, &crate::UpgradeLimits::default()).unwrap();
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&upgraded), PlaneSetLimits::default()).unwrap();
        assert_eq!(reader.image_directory(), Some(&images));
        check_crop(&mut reader, u64::MAX, &original, &op, stride);
    }
}

#[test]
fn primary_original_needs_no_plane_records_and_all_native_scalar_bits_survive_views() {
    let info = ImageInfo::new(7, 8, Channels::Gray, SampleType::U8);
    let source = pixels(&info, info.row_bytes());
    let primary = ImiEncoderConfig::lossless()
        .encode_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let mut builder = PlaneSetBuilder::new(&primary).unwrap();
    builder
        .set_image_directory(
            ImageDirectory::new(
                0,
                vec![ImageView::new(
                    0,
                    "original",
                    ImageViewRole::Original,
                    ViewBacking::Primary,
                )],
            )
            .unwrap(),
        )
        .unwrap();
    assert!(builder.dir.records.is_empty());
    let file = builder.finish().unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    check_crop(&mut reader, 0, &info, &source, info.row_bytes());
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
        let info = ImageInfo::new(17, 13, Channels::Gray, sample);
        let stride = info.row_bytes() + 3;
        let source = pixels(&info, stride);
        let mut builder = PlaneSetBuilder::new(&primary).unwrap();
        let p = builder
            .add_plane(PlaneDescriptor::new("native", sample))
            .unwrap();
        let level = builder
            .add_level(
                p,
                LevelDescriptor::new(17, 13, 7, 5),
                &source,
                stride,
                ImiEncoderConfig::lossless(),
            )
            .unwrap();
        builder
            .set_image_directory(
                ImageDirectory::new(
                    1,
                    vec![ImageView::new(
                        1,
                        "original",
                        ImageViewRole::Original,
                        ViewBacking::Level { plane: p, level },
                    )],
                )
                .unwrap(),
            )
            .unwrap();
        let file = builder.finish().unwrap();
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        check_crop(&mut reader, 1, &info, &source, stride);
    }
}

#[test]
fn native_views_bound_counts_resolve_ids_and_preserve_legacy_admission() {
    let one = ImageView::new(0, "original", ImageViewRole::Original, ViewBacking::Primary);
    assert!(ImageDirectory::new(1, vec![one.clone()]).is_err());
    assert!(ImageDirectory::new(0, vec![one.clone(), one.clone()]).is_err());
    let images = ImageDirectory::new(0, vec![one]).unwrap();
    let encoded = images.encode(PlaneSetLimits::default()).unwrap();
    assert_eq!(
        ImageDirectory::read(&encoded, PlaneSetLimits::default()).unwrap(),
        images
    );
    assert!(ImageDirectory::read(&encoded, PlaneSetLimits::default().with_views(0)).is_err());
    for end in 0..encoded.len() {
        assert!(ImageDirectory::read(&encoded[..end], PlaneSetLimits::default()).is_err())
    }
    let mut extra = encoded.clone();
    extra.push(0);
    assert!(ImageDirectory::read(&extra, PlaneSetLimits::default()).is_err());
    let info = ImageInfo::new(7, 8, Channels::Gray, SampleType::U8);
    let source = pixels(&info, info.row_bytes());
    let legacy = ImiEncoderConfig::lossless()
        .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let mut builder = PlaneSetBuilder::new(&legacy).unwrap();
    assert_eq!(
        builder.set_image_directory(images.clone()),
        Err(Error::InvalidInput("native image directory requires V4"))
    );
    let current = ImiEncoderConfig::lossless()
        .encode_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let mut builder = PlaneSetBuilder::new(&current).unwrap();
    assert!(
        builder
            .set_image_directory(
                ImageDirectory::new(
                    0,
                    vec![ImageView::new(
                        0,
                        "bad",
                        ImageViewRole::Original,
                        ViewBacking::Level {
                            plane: PlaneId(999),
                            level: 0
                        }
                    )]
                )
                .unwrap()
            )
            .is_err()
    );
    builder.set_image_directory(images).unwrap();
    assert!(builder.dir.images.is_some());
    let raw = builder.encode_directory().unwrap();
    let mut ids = Vec::new();
    crate::parse::records::head_records(&raw, |id, _| {
        ids.push(id);
        Ok(())
    })
    .unwrap();
    assert!(ids.contains(&101), "native view record missing: {ids:?}");
    assert!(matches!(
        wire::decode(&raw, PlaneSetLimits::default(), builder.dir.primary_len, 3),
        Err(Error::Unsupported("unknown plane directory record"))
    ));
}

#[test]
fn registration_is_exact_reduced_and_handles_full_signed_ranges() {
    assert_eq!(
        AxisRegistration::new(74, 38, -6, 9).unwrap().scale(),
        (37, 19)
    );
    assert_eq!(
        AxisRegistration::new(74, 38, -6, 9).unwrap().offset(),
        (-2, 3)
    );
    assert_eq!(
        AxisRegistration::new(1, 1, i64::MIN, 1 << 63)
            .unwrap()
            .offset(),
        (-1, 1)
    );
    assert_eq!(
        AxisRegistration::new(u64::MAX, 1, 0, u64::MAX)
            .unwrap()
            .offset(),
        (0, 1)
    );
    for (n, d, o) in [(0, 1, 1), (1, 0, 1), (1, 1, 0)] {
        assert!(AxisRegistration::new(n, d, 0, o).is_err())
    }
    let registration = ViewRegistration::resized(37, 29, 19, 11).unwrap();
    assert_eq!(registration.x.scale(), (37, 19));
    assert_eq!(registration.y.scale(), (29, 11));
}
