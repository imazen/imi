use imij::planes::{PlaneSetLimits, PlaneSetReader, Rect, SliceReader};
use imij::{FrameDecoder, FrameKind, UpgradeLimits, upgrade_image};

fn compare_frames(old: &[u8], new: &[u8]) {
    let mut a = FrameDecoder::new(old).unwrap();
    let mut b = FrameDecoder::new(new).unwrap();
    assert_eq!(a.info(), b.info());
    assert_eq!(a.kind(), b.kind());
    assert_eq!(a.loop_count(), b.loop_count());
    assert_eq!(a.frame_count(), b.frame_count());
    let animation = a.kind() == FrameKind::Animation;
    let mut left = vec![0; a.info().checked_buffer_len().unwrap()];
    let mut right = left.clone();
    let mut history: Vec<(Vec<u8>, usize)> = Vec::new();
    while let Some(header) = a.next_frame().unwrap() {
        assert_eq!(
            format!("{header:?}"),
            format!("{:?}", b.next_frame().unwrap().unwrap())
        );
        assert_eq!(a.frame_info(), b.frame_info());
        assert_eq!(a.frame_metadata(), b.frame_metadata());
        assert_eq!(a.reference_index(), b.reference_index());
        let info = if animation {
            a.info().clone()
        } else {
            a.frame_info().unwrap()
        };
        let stride = info.row_bytes();
        if !animation {
            left.resize(info.checked_buffer_len().unwrap(), 0);
            right.resize(left.len(), 0);
        }
        if let Some(index) = a.reference_index() {
            let (reference, rs) = &history[index];
            assert_eq!(*rs, stride);
            left.copy_from_slice(reference);
            right.copy_from_slice(reference);
        }
        a.decode_into(&mut left, stride, 1).unwrap();
        b.decode_into(&mut right, stride, 1).unwrap();
        assert_eq!(left, right);
        history.push((left.clone(), stride));
    }
    assert!(b.next_frame().unwrap().is_none());
}

#[test]
fn frozen_v2_framing_migrates_without_changing_frames_or_metadata() {
    for file in [
        include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/25-seed-csum-table-last.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/27-seed-csum-lossy.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/28-seed-csum-animation.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/29-seed-csum-previews.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/30-seed-csum-mixed-pages.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/33-seed-v2-fram-records.imi").as_slice(),
        include_bytes!("../../fuzz/seeds/36-seed-v2-unknown-header-record-even.imi").as_slice(),
    ] {
        assert_eq!(imij::header(file).unwrap().version, 2);
        let new = upgrade_image(file, &UpgradeLimits::default()).unwrap();
        assert_eq!(imij::header(&new).unwrap().version, 4);
        compare_frames(file, &new);
        assert_eq!(upgrade_image(&new, &UpgradeLimits::default()).unwrap(), new);
    }
}

#[test]
fn frozen_plane_directory_and_tiles_relocate_and_decode_identically() {
    let old = include_bytes!("../../fuzz/seeds/34-seed-v2-plane-directory.imi");
    let new = upgrade_image(old, &UpgradeLimits::default()).unwrap();
    assert_eq!(imij::header(&new).unwrap().version, 4);
    compare_frames(old, &new);
    let mut a = PlaneSetReader::open(SliceReader::new(old), PlaneSetLimits::default()).unwrap();
    let mut b = PlaneSetReader::open(SliceReader::new(&new), PlaneSetLimits::default()).unwrap();
    assert_eq!(a.planes(), b.planes());
    assert_eq!(a.attachments(), b.attachments());
    for (p, desc) in a.planes().to_vec().iter().enumerate() {
        let pa = a.plane_id(p).unwrap();
        let pb = b.plane_id(p).unwrap();
        for (level, d) in desc.levels().iter().enumerate() {
            let stride = d.width() as usize * desc.sample_type().bytes();
            let mut left = vec![0; stride * d.height() as usize];
            let mut right = left.clone();
            let rect = Rect::new(0, 0, d.width(), d.height());
            a.decode_region(pa, level as u32, rect, &mut left, stride)
                .unwrap();
            b.decode_region(pb, level as u32, rect, &mut right, stride)
                .unwrap();
            assert_eq!(left, right);
        }
    }
    for index in 0..a.attachments().len() {
        assert_eq!(
            a.read_attachment(a.attachment_id(index).unwrap()).unwrap(),
            b.read_attachment(b.attachment_id(index).unwrap()).unwrap()
        );
    }
}

#[test]
fn checksum_and_size_failures_do_not_produce_an_upgrade() {
    let file = include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi");
    let mut limits = UpgradeLimits::default();
    limits.max_encoded_bytes = file.len() - 1;
    assert_eq!(upgrade_image(file, &limits), Err(imij::Error::TooLarge));
    let mut corrupt = file.to_vec();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(upgrade_image(&corrupt, &UpgradeLimits::default()).is_err());
    let unknown = include_bytes!("../../fuzz/seeds/26-seed-csum-unknown-algo.imi");
    assert!(matches!(
        upgrade_image(unknown, &UpgradeLimits::default()),
        Err(imij::Error::Unsupported(_))
    ));
}

#[test]
fn gain_map_and_preview_images_are_upgraded_inside_a_current_outer_file() {
    let nested = include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi").to_vec();
    let metadata = imij::Metadata {
        gain_map: Some(imij::GainMap {
            params: imij::GainMapParams::default(),
            alt_cicp: Some(imij::Cicp::SRGB),
            alt_icc: Some(b"opaque profile".to_vec()),
            image: nested.clone(),
        }),
        ..Default::default()
    };
    let info = imij::ImageInfo::new(1, 1, imij::Channels::Gray, imij::SampleType::U8);
    let file = imij::ImiEncoderConfig::default()
        .with_checksum(false)
        .encode_samples(imij::ImageRef::new(&[42], 1, &info), &metadata)
        .unwrap();
    let (header, mut chunks) = common::parts(&file);
    chunks.insert(0, (*b"pRVW", nested.clone()));
    let file = common::join(&header, &chunks, false);
    let new = upgrade_image(&file, &UpgradeLimits::default()).unwrap();
    assert_eq!(imij::decode(&file).unwrap(), imij::decode(&new).unwrap());
    let (_, got) = imij::probe(&new).unwrap();
    let gain = got.gain_map.unwrap();
    assert_eq!(gain.params, metadata.gain_map.as_ref().unwrap().params);
    assert_eq!(gain.alt_cicp, metadata.gain_map.as_ref().unwrap().alt_cicp);
    assert_eq!(gain.alt_icc, metadata.gain_map.as_ref().unwrap().alt_icc);
    assert_eq!(imij::header(&gain.image).unwrap().version, 4);
    assert_eq!(
        imij::decode(&gain.image).unwrap(),
        imij::decode(&nested).unwrap()
    );
    let previews = imij::previews(&new).unwrap();
    assert_eq!(previews.len(), 1);
    assert_eq!(imij::header(previews[0]).unwrap().version, 4);
    assert_eq!(
        imij::decode(previews[0]).unwrap(),
        imij::decode(&nested).unwrap()
    );
    let mut limits = UpgradeLimits::default();
    limits.max_depth = 0;
    assert_eq!(upgrade_image(&file, &limits), Err(imij::Error::TooLarge));
}

#[test]
fn summed_plane_set_rebuilds_offsets_and_keeps_attachment_bytes() {
    use imij::planes::*;
    let primary = include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi");
    let mut builder = PlaneSetBuilder::new(primary).unwrap();
    let plane = builder
        .add_plane(PlaneDescriptor::new("depth", imij::SampleType::U8))
        .unwrap();
    builder
        .add_level(
            plane,
            LevelDescriptor::new(2, 2, 1, 1),
            &[1, 8, 3, 9],
            2,
            imij::ImiEncoderConfig::default().with_effort(0.0),
        )
        .unwrap();
    let attachment = builder
        .add_attachment(
            AttachmentDescriptor::new("note", "application/octet-stream"),
            b"kept",
        )
        .unwrap();
    let file = builder.finish().unwrap();
    let new = upgrade_image(&file, &UpgradeLimits::default()).unwrap();
    assert_eq!(imij::decode(&file).unwrap(), imij::decode(&new).unwrap());
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&new), PlaneSetLimits::default()).unwrap();
    let mut pixels = [0; 4];
    reader
        .decode_region(plane, 0, Rect::new(0, 0, 2, 2), &mut pixels, 2)
        .unwrap();
    assert_eq!(pixels, [1, 8, 3, 9]);
    assert_eq!(reader.read_attachment(attachment).unwrap(), b"kept");
    assert!(reader.checksum_coverage().checking);
}

mod common;
#[test]
fn reserved_and_trailing_unit_tables_keep_exact_unit_spans() {
    use common::unit_table::{Form, with_form};
    let (info, pixels) =
        imij::decode(include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi")).unwrap();
    let base = imij::ImiEncoderConfig::lossless()
        .with_checksum(false)
        .tune_lossless(|t| {
            t.stripe_rows = Some(2);
            t.palette = false;
        })
        .encode_legacy_samples(imij::ImageRef::packed(&pixels, &info), &Default::default())
        .unwrap();
    for form in [
        Form::Hint,
        Form::Trailing,
        Form::Reserved(8),
        Form::Empty(8),
        Form::Tail(8),
    ] {
        let mut old = with_form(&base, form);
        let spans = common::unit_table::spans(&old);
        let range = common::data_payload(&old);
        let h = imij::header(&old).unwrap();
        for (id, whole, _) in common::payload(&old[range.clone()], &h.info, false).units {
            old[range.start + whole.start] = match id {
                59 => 9,
                78 => 8,
                _ => panic!("unexpected lossless unit"),
            };
        }
        let version = common::header_record(&old).start;
        old[version] = 2;
        let expected = imij::decode(&old).unwrap();
        let new = upgrade_image(&old, &UpgradeLimits::default()).unwrap();
        assert_eq!(imij::decode(&new).unwrap(), expected);
        // V4 wraps each unchanged entropy payload in an identified, locally
        // checked object. Recover the old framing lengths independently so
        // the original exact-span assertion remains meaningful.
        let data = &new[common::data_payload(&new)];
        let (setup_len, n) = common::varint(data);
        let setup = &data[n..n + setup_len as usize];
        let mut si = 0;
        let mut mode = 0;
        for _ in 0..4 {
            let (v, k) = common::varint(&setup[si..]);
            si += k;
            mode = v;
        }
        let trailer = if mode == 0 { 0 } else { 16 };
        let mut at = n + setup_len as usize;
        let (placement, k) = common::varint(&data[at..]);
        at += k;
        assert_eq!(placement, 2);
        let (capacity, k) = common::varint(&data[at..]);
        at += k;
        let slot = &data[at..at + capacity as usize];
        let (active, k) = common::varint(slot);
        assert!(active > 0);
        assert!(slot[k + active as usize..].iter().all(|&byte| byte == 0));
        at += capacity as usize;
        let mut recovered = Vec::new();
        while at < data.len() {
            let (kind, k) = common::varint(&data[at..]);
            at += k;
            let (len, k) = common::varint(&data[at..]);
            at += k;
            let body = &data[at..at + len as usize - trailer];
            if kind == 59 {
                let (_, id_len) = common::varint(body);
                let entropy = &body[id_len..];
                let mut length = Vec::new();
                common::put_varint(&mut length, entropy.len() as u64);
                recovered.push((1 + length.len() + entropy.len()) as u64);
            }
            at += len as usize;
        }
        assert_eq!(spans, recovered);
    }
}

#[test]
fn plane_directory_limits_are_explicit_and_configurable() {
    let old = include_bytes!("../../fuzz/seeds/34-seed-v2-plane-directory.imi");
    let mut limits = UpgradeLimits::default();
    limits.plane_limits = limits.plane_limits.with_index_bytes(1);
    assert!(upgrade_image(old, &limits).is_err());
    limits.plane_limits = imij::planes::PlaneSetLimits::default();
    assert_eq!(
        imij::decode(&upgrade_image(old, &limits).unwrap()).unwrap(),
        imij::decode(old).unwrap()
    );
}

#[test]
fn plane_primary_also_obeys_record_admission_limit() {
    let primary = include_bytes!("../../fuzz/seeds/24-seed-csum-lossless.imi");
    let set = imij::planes::PlaneSetBuilder::new(primary)
        .unwrap()
        .finish()
        .unwrap();
    let mut limits = UpgradeLimits::default();
    limits.plane_limits = limits.plane_limits.with_record_bytes(1);
    assert!(matches!(
        upgrade_image(&set, &limits),
        Err(imij::Error::TooLarge)
    ));
    limits.plane_limits = PlaneSetLimits::default().with_file_bytes(set.len() as u64 - 1);
    assert!(matches!(
        upgrade_image(&set, &limits),
        Err(imij::Error::TooLarge)
    ));
}
