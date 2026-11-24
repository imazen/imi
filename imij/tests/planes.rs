use imij::planes::*;
use imij::{Channels, Cicp, Error, ImageInfo, ImiEncoderConfig, Metadata, SampleType};

fn lossless() -> ImiEncoderConfig {
    imij::ImiEncoderConfig::default().with_effort(0.0)
}
fn primary() -> Vec<u8> {
    imij::ImiEncoderConfig::default()
        .with_effort(0.0)
        .encode_samples(
            imij::ImageRef::new(
                &[1, 2, 3, 4],
                2,
                &ImageInfo::new(2, 2, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap()
}
fn scalar_file() -> (Vec<u8>, PlaneId, Vec<u8>) {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let id = b
        .add_plane(
            PlaneDescriptor::new("depth", SampleType::U16)
                .with_role(PlaneRole::Depth)
                .with_units("m")
                .with_origin(-7, 11),
        )
        .unwrap();
    let pixels: Vec<u8> = (0u16..64).flat_map(|n| (n * 997).to_ne_bytes()).collect();
    b.add_level(
        id,
        LevelDescriptor::new(8, 8, 4, 4),
        &pixels,
        16,
        lossless(),
    )
    .unwrap();
    (b.finish().unwrap(), id, pixels)
}

#[test]
fn adding_planes_to_v2_primaries_keeps_the_directory_in_version_two() {
    for primary in [
        include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi").as_slice(),
    ] {
        assert_eq!(imij::header(primary).unwrap().version, 2);
        let original = imij::decode(primary).unwrap();
        let mut builder = PlaneSetBuilder::new(primary).unwrap();
        let plane = builder
            .add_plane(PlaneDescriptor::new("depth", SampleType::U8))
            .unwrap();
        builder
            .add_level(
                plane,
                LevelDescriptor::new(2, 2, 1, 1),
                &[7, 11, 19, 23],
                2,
                lossless(),
            )
            .unwrap();
        let attachment = builder
            .add_attachment(AttachmentDescriptor::new("note", "text/plain"), b"retained")
            .unwrap();
        let file = builder.finish().unwrap();
        assert_eq!(imij::header(&file).unwrap().version, 2);
        assert_eq!(imij::decode(&file).unwrap(), original);
        let mut reader =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        let mut pixels = [0; 4];
        reader
            .decode_region(plane, 0, Rect::new(0, 0, 2, 2), &mut pixels, 2)
            .unwrap();
        assert_eq!(pixels, [7, 11, 19, 23]);
        assert_eq!(reader.read_attachment(attachment).unwrap(), b"retained");
    }
}

#[test]
fn mixed_scalar_types_preserve_bits_roi_and_padding() {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let calibration = b
        .add_attachment(
            AttachmentDescriptor::new("calibration", "application/octet-stream").with_group(42),
            b"opaque-vendor\0payload",
        )
        .unwrap();
    let mut fixtures = Vec::new();
    #[cfg_attr(not(feature = "extended"), allow(unused_mut))]
    let mut types = vec![
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    #[cfg(feature = "extended")]
    types.extend([SampleType::U32, SampleType::I32]);
    for (i, sample) in types.into_iter().enumerate() {
        let id = b
            .add_plane(
                PlaneDescriptor::new(format!("plane-{i}"), sample)
                    .with_group(42)
                    .with_calibration(calibration)
                    .with_role(PlaneRole::Custom("vendor.sample".into())),
            )
            .unwrap();
        let patterns = [
            0u32, 0x80000000, 0x7f800000, 0xff800000, 1, 0x7fc01234, 0x7f801234, 0xffffffff,
            0x8000, 0x7c00, 0x7e55, 0x7c55,
        ];
        let tight: Vec<u8> = (0..35)
            .flat_map(|j| patterns[j % patterns.len()].to_ne_bytes()[..sample.bytes()].to_vec())
            .collect();
        let row = 7 * sample.bytes();
        let stride = row + 3;
        let mut padded = vec![0x9e; stride * 5];
        for y in 0..5 {
            padded[y * stride..y * stride + row].copy_from_slice(&tight[y * row..(y + 1) * row]);
        }
        b.add_level(
            id,
            LevelDescriptor::new(7, 5, 3, 2),
            &padded,
            stride,
            lossless(),
        )
        .unwrap();
        fixtures.push((id, sample, tight));
    }
    let file = b.finish().unwrap();
    // Ordinary fallback remains usable even with all ancillary records.
    assert_eq!(imij::decode(&file).unwrap().1, [1, 2, 3, 4]);
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    assert_eq!(
        r.read_attachment(calibration).unwrap(),
        b"opaque-vendor\0payload"
    );
    for (id, sample, tight) in fixtures {
        let row = 7 * sample.bytes();
        let mut full = vec![0; row * 5];
        r.decode_region(id, 0, Rect::new(0, 0, 7, 5), &mut full, row)
            .unwrap();
        assert_eq!(full, tight);
        let roirow = 4 * sample.bytes();
        let stride = roirow + 5;
        let mut crop = vec![0xa5; stride * 3 + 7];
        r.decode_region(id, 0, Rect::new(2, 1, 4, 3), &mut crop, stride)
            .unwrap();
        for y in 0..3 {
            assert_eq!(
                &crop[y * stride..y * stride + roirow],
                &tight[(y + 1) * row + 2 * sample.bytes()..(y + 1) * row + 6 * sample.bytes()]
            );
            assert!(
                crop[y * stride + roirow..(y + 1) * stride]
                    .iter()
                    .all(|&v| v == 0xa5)
            );
        }
        assert!(crop[stride * 3..].iter().all(|&v| v == 0xa5));
    }
}

#[test]
fn all_half_bits_exact_across_independent_tiles() {
    let pixels: Vec<u8> = (0..=u16::MAX).flat_map(u16::to_ne_bytes).collect();
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("half-bits", SampleType::F16))
        .unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(256, 256, 64, 64),
        &pixels,
        512,
        lossless(),
    )
    .unwrap();
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let mut out = vec![0; pixels.len()];
    r.decode_region(id, 0, Rect::new(0, 0, 256, 256), &mut out, 512)
        .unwrap();
    assert_eq!(out, pixels);
}

#[test]
fn packed_rgba_tiles_share_color_codec_and_exact_alpha() {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let ids: Vec<_> = ["red", "green", "blue", "alpha"]
        .into_iter()
        .map(|n| {
            b.add_plane(PlaneDescriptor::new(n, SampleType::U8))
                .unwrap()
        })
        .collect();
    let pixels: Vec<u8> = (0..11 * 9 * 4)
        .map(|i| ((i * 137 + i / 7) % 256) as u8)
        .collect();
    b.add_color_level(
        &ids,
        LevelDescriptor::new(11, 9, 4, 5),
        &pixels,
        44,
        Channels::Rgba,
        lossless(),
    )
    .unwrap();
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    assert_eq!(r.planes()[0].levels()[0].tile_channels(), Channels::Rgba);
    let mut color = vec![0; 7 * 6 * 4];
    r.decode_color_region(&ids, 0, Rect::new(2, 1, 7, 6), &mut color, 28)
        .unwrap();
    for y in 0..6 {
        assert_eq!(
            &color[y * 28..(y + 1) * 28],
            &pixels[(y + 1) * 44 + 8..(y + 1) * 44 + 36]
        );
    }
    for (c, &id) in ids.iter().enumerate() {
        let mut scalar = vec![0; 99];
        r.decode_region(id, 0, Rect::new(0, 0, 11, 9), &mut scalar, 11)
            .unwrap();
        assert_eq!(
            scalar,
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[c])
                .collect::<Vec<_>>()
        );
    }
    let mut invalid = vec![0; 7 * 6 * 4];
    let mut swapped = ids.clone();
    swapped.swap(0, 1);
    assert!(
        r.decode_color_region(&swapped, 0, Rect::new(2, 1, 7, 6), &mut invalid, 28)
            .is_err()
    );
}

#[test]
fn arbitrary_levels_can_mix_lossless_and_lossy() {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("display", SampleType::U8))
        .unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(7, 5, 7, 5),
        &[79; 35],
        7,
        lossless(),
    )
    .unwrap();
    let pixels: Vec<u8> = (0..15).map(|i| (i * 17) as u8).collect();
    let cfg = imij::ImiEncoderConfig::lossy(80.0).with_threads(1);
    b.add_level(
        id,
        LevelDescriptor::new(5, 3, 5, 3),
        &pixels,
        5,
        cfg.clone(),
    )
    .unwrap();
    let reference = cfg
        .encode_samples(
            imij::ImageRef::new(
                &pixels,
                5,
                &ImageInfo::new(5, 3, Channels::Gray, SampleType::U8),
            ),
            &Metadata::default(),
        )
        .unwrap();
    let expected = imij::decode(&reference).unwrap().1;
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let mut out = vec![0; 15];
    r.decode_region(id, 1, Rect::new(0, 0, 5, 3), &mut out, 5)
        .unwrap();
    assert_eq!(out, expected);
    let mut exact = vec![0; 35];
    r.decode_region(id, 0, Rect::new(0, 0, 7, 5), &mut exact, 7)
        .unwrap();
    assert_eq!(exact, vec![79; 35]);
}

struct CountReader<'a> {
    bytes: &'a [u8],
    reads: Vec<(u64, usize)>,
}
impl ReadAt for CountReader<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads.push((offset, out.len()));
        SliceReader::new(self.bytes).read_exact_at(offset, out)
    }
}

#[test]
fn seek_reads_index_then_only_requested_tile_and_attachment_range() {
    let (file, id, pixels) = scalar_file();
    let mut r = PlaneSetReader::open(
        CountReader {
            bytes: &file,
            reads: Vec::new(),
        },
        PlaneSetLimits::default(),
    )
    .unwrap();
    let mut crop = vec![0; 8];
    r.decode_region(id, 0, Rect::new(0, 0, 2, 2), &mut crop, 4)
        .unwrap();
    assert_eq!(&crop[..4], &pixels[..4]);
    assert_eq!(&crop[4..], &pixels[16..20]);
    let source = r.into_inner();
    assert_eq!(source.reads.len(), 4); // signature/footer/index + one complete framed record
    assert!(source.reads.iter().map(|(_, n)| n).sum::<usize>() < file.len());
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let timing = AttachmentTiming::new(-1, 30, 120, 30).unwrap();
    let clip: Vec<u8> = (0..10000).map(|i| (i % 251) as u8).collect();
    let a = b
        .add_attachment(
            AttachmentDescriptor::new("motion", "video/mp4").with_timing(timing),
            &clip,
        )
        .unwrap();
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(
        CountReader {
            bytes: &file,
            reads: Vec::new(),
        },
        PlaneSetLimits::default().with_work_bytes(128, 128),
    )
    .unwrap();
    assert_eq!(r.attachment_len(a).unwrap(), 10000);
    assert_eq!(r.attachments()[0].timing(), Some(timing));
    let mut out = [0u8; 37];
    r.read_attachment_into_range(a, 999, &mut out).unwrap();
    assert_eq!(&out, &clip[999..1036]);
    assert!(r.read_attachment(a).is_err());
    assert!(r.read_attachment_into_range(a, 9990, &mut out).is_err());
    let source = r.into_inner();
    assert_eq!(
        source.reads.last(),
        Some(&(source.reads[3].0 + source.reads[3].1 as u64 + 999, 37))
    );
}

#[test]
fn roi_limits_reject_before_fetch_and_leave_output_untouched() {
    let (file, id, _) = scalar_file();
    let mut r = PlaneSetReader::open(
        CountReader {
            bytes: &file,
            reads: Vec::new(),
        },
        PlaneSetLimits::default().with_work_bytes(u64::MAX, 31),
    )
    .unwrap();
    let mut out = [0x55; 8];
    assert!(
        r.decode_region(id, 0, Rect::new(0, 0, 2, 2), &mut out, 4)
            .is_err()
    );
    assert_eq!(out, [0x55; 8]);
    assert_eq!(r.into_inner().reads.len(), 3);
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    for rect in [
        Rect::new(0, 0, 0, 1),
        Rect::new(8, 0, 1, 1),
        Rect::new(u32::MAX, 0, 2, 1),
    ] {
        assert!(r.decode_region(id, 0, rect, &mut out, 4).is_err());
    }
    assert!(
        r.decode_region(id, 0, Rect::new(0, 0, 2, 2), &mut out, 3)
            .is_err()
    );
}

#[test]
fn malformed_locator_truncation_and_index_caps_are_bounded() {
    let (file, _, _) = scalar_file();
    for cut in [0, 7, 36, file.len() - 1, file.len() - 37] {
        assert!(
            PlaneSetReader::open(SliceReader::new(&file[..cut]), PlaneSetLimits::default())
                .is_err()
        );
    }
    for at in [
        file.len() - 37,
        file.len() - 37 + 4,
        file.len() - 37 + 9,
        file.len() - 1,
    ] {
        let mut bad = file.clone();
        bad[at] ^= 1;
        assert!(PlaneSetReader::open(SliceReader::new(&bad), PlaneSetLimits::default()).is_err());
    }
    let mut bad = file.clone();
    let at = bad.len() - 37 + 13;
    bad[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(PlaneSetReader::open(SliceReader::new(&bad), PlaneSetLimits::default()).is_err());
    let source = CountReader {
        bytes: &file,
        reads: Vec::new(),
    };
    assert!(PlaneSetReader::open(source, PlaneSetLimits::default().with_index_bytes(1)).is_err());
    assert!(
        PlaneSetReader::open(
            SliceReader::new(&file),
            PlaneSetLimits::default().with_records(1)
        )
        .is_err()
    );
    assert!(
        PlaneSetReader::open(
            SliceReader::new(&file),
            PlaneSetLimits::default().with_tile_bytes(1)
        )
        .is_err()
    );
}

#[test]
fn rejected_additions_preserve_existing_records() {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("raw", SampleType::U8).with_role(PlaneRole::Raw))
        .unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(2, 2, 1, 1),
        &[1, 2, 3, 4],
        2,
        lossless(),
    )
    .unwrap();
    assert!(
        b.add_level(
            id,
            LevelDescriptor::new(2, 2, 1, 1),
            &[1, 2, 3, 4],
            2,
            imij::ImiEncoderConfig::lossy(50.0)
        )
        .is_err()
    );
    assert!(
        b.add_level(id, LevelDescriptor::new(3, 3, 1, 1), &[0; 8], 3, lossless())
            .is_err()
    );
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let mut out = [0; 4];
    r.decode_region(id, 0, Rect::new(0, 0, 2, 2), &mut out, 2)
        .unwrap();
    assert_eq!(out, [1, 2, 3, 4]);
}

#[test]
fn hdr_planes_preserve_independent_profiles_and_reject_nonlinear_lossy_floats() {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let profile = b
        .add_attachment(
            AttachmentDescriptor::new("display-profile", "application/vnd.iccprofile"),
            b"exact-opaque-ICC",
        )
        .unwrap();
    let mut ids = Vec::new();
    for name in ["red", "green", "blue"] {
        ids.push(
            b.add_plane(
                PlaneDescriptor::new(name, SampleType::U16)
                    .with_cicp(Cicp::BT2100_PQ)
                    .with_icc_profile(profile),
            )
            .unwrap(),
        );
    }
    let values = [0u16, 65535, 32987, 17, 2100, 64999];
    let pixels: Vec<u8> = values.into_iter().flat_map(u16::to_ne_bytes).collect();
    b.add_color_level(
        &ids,
        LevelDescriptor::new(2, 1, 1, 1),
        &pixels,
        12,
        Channels::Rgb,
        lossless(),
    )
    .unwrap();
    let float = b
        .add_plane(
            PlaneDescriptor::new("nonlinear-float", SampleType::F16).with_cicp(Cicp::BT2100_HLG),
        )
        .unwrap();
    let bits = [0x00, 0x80, 0x55, 0x7e];
    assert!(
        b.add_level(
            float,
            LevelDescriptor::new(2, 1, 1, 1),
            &bits,
            4,
            imij::ImiEncoderConfig::lossy(50.0)
        )
        .is_err()
    );
    b.add_level(
        float,
        LevelDescriptor::new(2, 1, 1, 1),
        &bits,
        4,
        lossless(),
    )
    .unwrap();
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    assert_eq!(r.planes()[0].cicp(), Cicp::BT2100_PQ);
    assert_eq!(r.planes()[0].icc_profile(), Some(profile));
    assert_eq!(r.read_attachment(profile).unwrap(), b"exact-opaque-ICC");
    let mut out = [0; 12];
    r.decode_color_region(&ids, 0, Rect::new(0, 0, 2, 1), &mut out, 12)
        .unwrap();
    assert_eq!(out.as_slice(), pixels);
    let mut raw = [0; 4];
    r.decode_region(float, 0, Rect::new(0, 0, 2, 1), &mut raw, 4)
        .unwrap();
    assert_eq!(raw, bits);
}

#[test]
fn shared_color_levels_can_have_different_component_level_indices() {
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let ids: Vec<_> = ["red", "green", "blue"]
        .into_iter()
        .map(|n| {
            b.add_plane(PlaneDescriptor::new(n, SampleType::U8))
                .unwrap()
        })
        .collect();
    b.add_level(
        ids[0],
        LevelDescriptor::new(1, 1, 1, 1),
        &[23],
        1,
        lossless(),
    )
    .unwrap();
    let pixels = [1, 2, 3, 4, 5, 6];
    let indices = b
        .add_color_level(
            &ids,
            LevelDescriptor::new(2, 1, 1, 1),
            &pixels,
            6,
            Channels::Rgb,
            lossless(),
        )
        .unwrap();
    assert_eq!(indices, [1, 0, 0]);
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let components: Vec<_> = ids.iter().copied().zip(indices).collect();
    let mut out = [0; 6];
    r.decode_color_levels(&components, Rect::new(0, 0, 2, 1), &mut out, 6)
        .unwrap();
    assert_eq!(out, pixels);
}
