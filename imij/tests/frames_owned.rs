//! Owned frame input retains the borrowed renderer's pixels and admission.
use imij::{
    Channels, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, FrameKind, ImageInfo,
    ImageReference, Limits, Metadata, SampleType, SequenceEncoder, SequenceItem, SequenceKind,
};

fn config() -> FrameConfig {
    FrameConfig::new({
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(4.0);
        cfg = cfg.with_threads(1);
        cfg
    })
}

fn random(info: &ImageInfo, mut state: u32) -> Vec<u8> {
    (0..info.checked_buffer_len().unwrap())
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

fn metadata(index: usize) -> Metadata {
    Metadata {
        exif: Some(vec![index as u8]),
        ..Default::default()
    }
}

fn padded(info: &ImageInfo, pixels: &[u8], padding: usize) -> (Vec<u8>, usize) {
    let row = info.row_bytes();
    let stride = row + padding;
    let mut out = vec![0xa5; stride * info.height as usize];
    for y in 0..info.height as usize {
        out[y * stride..y * stride + row].copy_from_slice(&pixels[y * row..(y + 1) * row]);
    }
    (out, stride)
}

fn compare(file: Vec<u8>, expected: &[(ImageInfo, Vec<u8>)], references: &[Option<usize>]) {
    fn owned_send_static<T: Send + 'static>(value: T) -> T {
        value
    }
    let checked = imij::add_pixel_checksum(file).unwrap();
    let mut borrowed = FrameDecoder::with_limits(&checked, &Limits::DEFAULT).unwrap();
    // The constructor owns an allocation distinct from the borrowed decoder's input.
    let mut owned =
        owned_send_static(FrameDecoder::from_owned(checked.clone(), &Limits::DEFAULT).unwrap());
    assert_eq!(owned.info(), borrowed.info());
    assert_eq!(owned.kind(), borrowed.kind());
    assert_eq!(owned.loop_count(), borrowed.loop_count());
    assert_eq!(owned.frame_count(), expected.len());
    let canvas_info = owned.info().clone();
    let (mut canvas, canvas_stride) = padded(
        &canvas_info,
        &vec![0; canvas_info.checked_buffer_len().unwrap()],
        17,
    );
    for (index, (info, pixels)) in expected.iter().enumerate() {
        let a = borrowed.next_frame().unwrap().unwrap();
        let b = owned.next_frame().unwrap().unwrap();
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
        assert_eq!(owned.frame_info(), borrowed.frame_info());
        assert_eq!(owned.frame_metadata(), borrowed.frame_metadata());
        assert_eq!(owned.reference_index(), references[index]);
        assert_eq!(borrowed.reference_index(), references[index]);
        let animation = owned.kind() == FrameKind::Animation;
        let (mut left, stride) = if animation {
            (canvas.clone(), canvas_stride)
        } else {
            padded(info, &vec![0; pixels.len()], 17)
        };
        let mut right = left.clone();
        if !animation && let Some(reference) = references[index] {
            let (reference, ref_stride) = padded(info, &expected[reference].1, 9);
            borrowed
                .decode_into_with_reference(&reference, ref_stride, &mut left, stride, 1)
                .unwrap();
            owned
                .decode_into_with_reference(&reference, ref_stride, &mut right, stride, 2)
                .unwrap();
        } else {
            borrowed.decode_into(&mut left, stride, 1).unwrap();
            owned.decode_into(&mut right, stride, 2).unwrap();
        }
        assert_eq!(left, right);
        for y in 0..info.height as usize {
            let row = info.row_bytes();
            assert_eq!(
                &right[y * stride..y * stride + row],
                &pixels[y * row..(y + 1) * row]
            );
            assert!(
                right[y * stride + row..(y + 1) * stride]
                    .iter()
                    .all(|&b| b == 0xa5)
            );
        }
        assert_eq!(
            owned.pixel_checksum_status(),
            imij::PixelChecksumStatus::Verified
        );
        assert_eq!(
            borrowed.pixel_checksum_status(),
            imij::PixelChecksumStatus::Verified
        );
        if animation {
            canvas = right;
        } else {
            assert_eq!(
                owned.frame_metadata().unwrap().exif,
                Some(vec![index as u8])
            );
        }
    }
    assert!(borrowed.next_frame().unwrap().is_none());
    assert!(owned.next_frame().unwrap().is_none());
}

#[test]
fn owned_legacy_cropped_and_empty_animation() {
    let info = ImageInfo::new(129, 33, Channels::Rgb, SampleType::U8);
    let first = random(&info, 711);
    let mut changed = first.clone();
    changed[info.row_bytes() * 7 + 3 * 11] ^= 3;
    let frames = vec![first, changed.clone(), changed];
    let mut encoder = FrameEncoder::animation(info.clone(), metadata(0), config());
    for pixels in &frames {
        encoder
            .push_frame(pixels, info.row_bytes(), FrameDuration { num: 1, den: 30 })
            .unwrap();
    }
    let file = encoder.finish().unwrap();
    let mut witness = FrameDecoder::new(&file).unwrap();
    witness.next_frame().unwrap().unwrap();
    let crop = witness.next_frame().unwrap().unwrap();
    assert!(crop.width < info.width && crop.height < info.height);
    let empty = witness.next_frame().unwrap().unwrap();
    assert!(empty.coding.is_none());
    assert!(empty.uses_previous);
    let refs = [None, crop.uses_previous.then_some(0), Some(1)];
    let expected: Vec<_> = frames
        .into_iter()
        .map(|pixels| (info.clone(), pixels))
        .collect();
    compare(file, &expected, &refs);
}

#[test]
fn owned_mixed_pages_keep_native_samples_and_item_metadata() {
    let infos = [
        ImageInfo::new(17, 9, Channels::Rgb, SampleType::U8),
        ImageInfo::new(3, 7, Channels::Gray, SampleType::F32),
    ];
    let mut encoder = FrameEncoder::mixed_pages(config());
    let mut expected = Vec::new();
    for (index, info) in infos.into_iter().enumerate() {
        let pixels = random(&info, 77 + index as u32);
        encoder
            .push_page_with(
                &pixels,
                info.row_bytes(),
                &info,
                &metadata(index),
                &config().coding,
            )
            .unwrap();
        expected.push((info, pixels));
    }
    compare(encoder.finish().unwrap(), &expected, &[None, None]);
}

#[test]
fn owned_unified_pages_retain_nonprevious_reference() {
    let info = ImageInfo::new(129, 33, Channels::Rgba, SampleType::U8);
    let first = random(&info, 711);
    let unrelated = random(&info, 983);
    let mut changed = first.clone();
    changed[13] ^= 3;
    let pixels = [first, unrelated, changed];
    let mut encoder = SequenceEncoder::new(SequenceKind::Pages, config());
    for (index, image) in pixels.iter().enumerate() {
        let meta = metadata(index);
        let mut item = SequenceItem::new(image, info.row_bytes(), &info).with_metadata(&meta);
        if index == 2 {
            item = item.with_reference(ImageReference {
                index: 0,
                pixels: &pixels[0],
                stride: info.row_bytes(),
            });
        }
        encoder.push(item).unwrap();
    }
    let expected: Vec<_> = pixels
        .into_iter()
        .map(|pixels| (info.clone(), pixels))
        .collect();
    compare(encoder.finish().unwrap(), &expected, &[None, None, Some(0)]);
}

#[test]
fn owned_constructor_matches_borrowed_admission() {
    let info = ImageInfo::new(17, 9, Channels::Gray, SampleType::U8);
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&random(&info, 71), info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    for data in [Vec::new(), file[..file.len() / 2].to_vec(), file.clone()] {
        for limits in [
            Limits::DEFAULT,
            {
                let mut limits = Limits::DEFAULT;
                limits.max_pixels = 1;
                limits
            },
            {
                let mut limits = Limits::DEFAULT;
                limits.max_bytes = 1;
                limits
            },
        ] {
            let borrowed = FrameDecoder::with_limits(&data, &limits).map(|_| ());
            let owned = FrameDecoder::from_owned(data.clone(), &limits).map(|_| ());
            assert_eq!(owned, borrowed);
        }
    }
}
