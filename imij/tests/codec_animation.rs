//! The common animation adapter preserves native pixels, timing and failures.
use imij::codec::{DecodeConfig, DecodeError, DecodedFrameMetadata};
use imij::{
    Channels, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, ImageInfo,
    ImageReference, Metadata, SampleType, SequenceEncoder, SequenceItem, SequenceKind,
};
use std::borrow::Cow;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use zencodec::decode::{AnimationFrameDecoder, DecodeJob, DecodeRowSink, DecoderConfig};
use zencodec::{ResourceLimits, StopToken};
use zenpixels::{PixelDescriptor, PixelSlice, PixelSliceMut};

fn config() -> FrameConfig {
    FrameConfig::new({
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(4.0);
        cfg = cfg.with_threads(1);
        cfg
    })
}

fn native_composited(file: &[u8]) -> Vec<Vec<u8>> {
    let mut decoder = FrameDecoder::new(file).unwrap();
    let info = decoder.info().clone();
    let mut canvas = vec![0; info.checked_buffer_len().unwrap()];
    let mut frames: Vec<Vec<u8>> = Vec::new();
    while decoder.next_frame().unwrap().is_some() {
        // Legacy updates use the retained canvas. Unified items may select
        // an earlier full raster instead of the immediately preceding frame.
        if let Some(index) = decoder.reference_index() {
            canvas.copy_from_slice(&frames[index]);
        }
        decoder
            .decode_into(&mut canvas, info.row_bytes(), 1)
            .unwrap();
        assert_eq!(
            decoder.pixel_checksum_status(),
            imij::PixelChecksumStatus::Verified
        );
        frames.push(canvas.clone());
    }
    frames
}

#[test]
fn lossy_legacy_compositing_matches_native_and_keeps_exact_alpha() {
    let info = ImageInfo::new(129, 41, Channels::Rgba, SampleType::U8);
    let first = pixels(&info, 633);
    let mut second = first.clone();
    for y in 9..17 {
        for x in 11..23 {
            let at = y * info.row_bytes() + x * 4;
            second[at] ^= 31;
            second[at + 3] ^= 7;
        }
    }
    let sources = [first, second.clone(), second];
    let mut lossy = imij::ImiEncoderConfig::lossy(60.0).with_effort(0.0);
    lossy = lossy.with_threads(1);
    let mut encoder = FrameEncoder::animation(info.clone(), metadata(0), FrameConfig::new(lossy));
    for source in &sources {
        encoder
            .push_frame(source, info.row_bytes(), FrameDuration { num: 1, den: 30 })
            .unwrap();
    }
    let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
    let expected = native_composited(&file);
    assert_ne!(
        expected[0], sources[0],
        "fixture must exercise lossy reconstruction"
    );
    let mut decoder = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Owned(file), &[])
        .unwrap();
    for (index, frame) in expected.iter().enumerate() {
        let actual = decoder.render_next_frame(None).unwrap().unwrap();
        assert_pixels(actual.pixels(), &info, frame);
        for y in 0..info.height {
            for x in 0..info.width as usize {
                assert_eq!(
                    actual.pixels().row(y)[x * 4 + 3],
                    sources[index][y as usize * info.row_bytes() + x * 4 + 3]
                );
            }
        }
    }
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[test]
fn lossy_unified_items_use_returned_reconstructed_earlier_reference() {
    let info = ImageInfo::new(129, 41, Channels::Rgb, SampleType::U8);
    let source = pixels(&info, 633);
    let unrelated = pixels(&info, 987);
    let mut lossy = imij::ImiEncoderConfig::lossy(60.0);
    lossy = lossy.with_threads(1);
    let mut encoder = SequenceEncoder::new(SequenceKind::Animation, FrameConfig::new(lossy));
    let first = encoder
        .push(SequenceItem::new(&source, info.row_bytes(), &info).with_metadata(&metadata(0)))
        .unwrap();
    assert_ne!(first, source);
    let second = encoder
        .push(SequenceItem::new(&unrelated, info.row_bytes(), &info).with_metadata(&metadata(1)))
        .unwrap();
    let third = encoder
        .push(
            SequenceItem::new(&first, info.row_bytes(), &info)
                .with_metadata(&metadata(2))
                .with_reference(ImageReference {
                    index: 0,
                    pixels: &first,
                    stride: info.row_bytes(),
                }),
        )
        .unwrap();
    let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
    let mut witness = FrameDecoder::new(&file).unwrap();
    for _ in 0..3 {
        witness.next_frame().unwrap().unwrap();
    }
    assert_eq!(
        witness.reference_index(),
        Some(0),
        "fixture must actually select nonprevious prediction"
    );
    let native = native_composited(&file);
    assert_eq!(native, vec![first, second, third]);
    let mut decoder = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Owned(file), &[])
        .unwrap();
    for (index, expected) in native.iter().enumerate() {
        let frame = decoder.render_next_frame_owned(None).unwrap().unwrap();
        assert_pixels(&frame.pixels(), &info, expected);
        let extra = frame.extras::<DecodedFrameMetadata>().unwrap();
        assert_eq!(extra.metadata, metadata(index));
        assert_eq!(extra.reference_index, (index == 2).then_some(0));
    }
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[test]
fn cooperative_render_cancellation_after_multiple_checks_is_sticky() {
    struct CountStop(AtomicUsize);
    impl zencodec::enough::Stop for CountStop {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.fetch_add(1, Ordering::SeqCst) + 1 >= 5 {
                Err(zencodec::enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    let (file, _, _) = unified(SampleType::U8);
    let mut decoder = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Owned(file), &[])
        .unwrap();
    let stop = CountStop(AtomicUsize::new(0));
    assert!(matches!(
        decoder.render_next_frame(Some(&how_far::StopOnly::borrowed(&stop))),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert_eq!(stop.0.load(Ordering::SeqCst), 5);
    let mut sink = Sink::default();
    assert!(decoder.render_next_frame_to_sink(None, &mut sink).is_err());
    assert_eq!((sink.begins, sink.finishes), (0, 0));
    assert!(decoder.render_next_frame_owned(None).is_err());
}

fn pixels(info: &ImageInfo, mut state: u32) -> Vec<u8> {
    if info.sample == SampleType::F32 {
        let bits = [
            0u32, 0x80000000, 1, 0x3f800000, 0x7f800000, 0xff800000, 0x7f800001, 0x7fc12abc,
        ];
        return (0..info.checked_buffer_len().unwrap() / 4)
            .flat_map(|i| bits[(i + state as usize) % bits.len()].to_ne_bytes())
            .collect();
    }
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
    let mut values = imij::item_metadata::ItemMetadata::default();
    values.filename = Some(format!("frame-{index}"));
    values.attributes.insert(
        "index".into(),
        imij::item_metadata::Value::Unsigned(index as u64),
    );
    let mut meta = Metadata {
        orientation: 1,
        exif: Some(vec![index as u8]),
        xmp: Some(b"<frame/>".to_vec()),
        ..Default::default()
    };
    values.write_metadata(&mut meta).unwrap();
    meta
}

fn assert_pixels(actual: &PixelSlice<'_>, info: &ImageInfo, expected: &[u8]) {
    assert_eq!((actual.width(), actual.rows()), (info.width, info.height));
    assert_eq!(
        actual.descriptor().bytes_per_pixel(),
        info.bytes_per_pixel()
    );
    for y in 0..info.height {
        let row = info.row_bytes();
        assert_eq!(
            actual.row(y),
            &expected[y as usize * row..(y as usize + 1) * row]
        );
    }
}

fn unified(sample: SampleType) -> (Vec<u8>, ImageInfo, Vec<Vec<u8>>) {
    let info = ImageInfo::new(129, 33, Channels::Rgb, sample);
    let first = pixels(&info, 711);
    let second = pixels(&info, 983);
    let mut third = first.clone();
    if sample == SampleType::F32 {
        third[12..16].copy_from_slice(&0.5f32.to_ne_bytes());
    } else {
        third[13] ^= 3;
    }
    let images = vec![first, second, third];
    let durations = [
        FrameDuration { num: 1, den: 30 },
        FrameDuration::ZERO,
        FrameDuration { num: 1, den: 2000 },
    ];
    let mut cfg = config();
    cfg.loop_count = 3;
    let mut encoder = SequenceEncoder::new(SequenceKind::Animation, cfg);
    for (i, image) in images.iter().enumerate() {
        let meta = metadata(i);
        let mut item = SequenceItem::new(image, info.row_bytes(), &info)
            .with_metadata(&meta)
            .with_duration(durations[i]);
        if i == 2 && sample != SampleType::F32 {
            item = item.with_reference(ImageReference {
                index: 0,
                pixels: &images[0],
                stride: info.row_bytes(),
            });
        }
        encoder.push(item).unwrap();
    }
    (
        imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap(),
        info,
        images,
    )
}

#[test]
// Also checks zencodec's deprecated millisecond accessor, which callers still use.
#[allow(deprecated)]
fn native_frames_own_input_preserve_timing_metadata_and_color_context() {
    fn send_static<T: Send + 'static>(value: T) -> T {
        value
    }
    assert!(DecodeConfig::capabilities().animation());
    for sample in [SampleType::U8, SampleType::U16, SampleType::F32] {
        let (file, info, images) = unified(sample);
        for data in [Cow::Borrowed(file.as_slice()), Cow::Owned(file.clone())] {
            let mut decoder = send_static(
                DecodeConfig::default()
                    .job()
                    .animation_frame_decoder(data, &[])
                    .unwrap(),
            );
            assert_eq!(decoder.frame_count(), Some(3));
            assert_eq!(decoder.loop_count(), Some(3));
            assert!(matches!(
                decoder.info().sequence,
                zencodec::ImageSequence::Animation {
                    frame_count: Some(3),
                    loop_count: Some(3),
                    ..
                }
            ));
            let mut previous_work = decoder.usage().work_units;
            for (i, expected) in images.iter().enumerate() {
                let owned = decoder.render_next_frame_owned(None).unwrap().unwrap();
                assert_pixels(&owned.pixels(), &info, expected);
                assert_eq!(owned.frame_index(), i as u32);
                assert_eq!(owned.duration_ms(), [33, 0, 0][i]);
                let extras = owned.extras::<DecodedFrameMetadata>().unwrap();
                assert_eq!(
                    owned.duration(),
                    zencodec::animation::FrameDuration::new(
                        extras.duration.num,
                        extras.duration.den
                    )
                    .unwrap()
                );
                assert_eq!(extras.image, info);
                assert_eq!(extras.metadata, metadata(i));
                assert_eq!(
                    extras.duration,
                    [
                        FrameDuration { num: 1, den: 30 },
                        FrameDuration::ZERO,
                        FrameDuration { num: 1, den: 2000 }
                    ][i]
                );
                assert_eq!(
                    extras.reference_index,
                    (i == 2 && sample != SampleType::F32).then_some(0)
                );
                let current = decoder.frame_metadata().unwrap();
                assert_eq!(current.metadata, extras.metadata);
                assert_eq!(current.duration, extras.duration);
                assert!(owned.pixels().color_context().is_some());
                assert!(decoder.usage().work_units > previous_work);
                previous_work = decoder.usage().work_units;
            }
            assert!(decoder.render_next_frame(None).unwrap().is_none());
            assert!(decoder.render_next_frame_owned(None).unwrap().is_none());
        }
        // Borrowed and owned rendering must retain the same actual color context.
        let mut borrowed = DecodeConfig::default()
            .job()
            .animation_frame_decoder(Cow::Borrowed(&file), &[])
            .unwrap();
        let mut owned = DecodeConfig::default()
            .job()
            .animation_frame_decoder(Cow::Owned(file.clone()), &[])
            .unwrap();
        let a = borrowed.render_next_frame(None).unwrap().unwrap();
        let b = owned.render_next_frame_owned(None).unwrap().unwrap();
        assert_eq!(a.pixels().color_context(), b.pixels().color_context());
    }
    // The executor survives destruction of the caller's borrowed allocation.
    let mut decoder = {
        let (file, _, _) = unified(SampleType::U8);
        send_static(
            DecodeConfig::default()
                .job()
                .animation_frame_decoder(Cow::Borrowed(&file), &[])
                .unwrap(),
        )
    };
    assert!(decoder.render_next_frame(None).unwrap().is_some());
}

#[test]
// Also checks zencodec's deprecated millisecond accessor, which callers still use.
#[allow(deprecated)]
fn legacy_crop_and_empty_updates_are_full_displayed_frames() {
    let info = ImageInfo::new(129, 33, Channels::Rgb, SampleType::U8);
    let first = pixels(&info, 711);
    let mut changed = first.clone();
    changed[info.row_bytes() * 7 + 33] ^= 3;
    let images = [first, changed.clone(), changed];
    let mut encoder = FrameEncoder::animation(info.clone(), metadata(0), config());
    for image in &images {
        encoder
            .push_frame(image, info.row_bytes(), FrameDuration { num: 1, den: 30 })
            .unwrap();
    }
    let file = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
    let mut witness = FrameDecoder::new(&file).unwrap();
    witness.next_frame().unwrap();
    let crop = witness.next_frame().unwrap().unwrap();
    assert!(crop.width < info.width && crop.height < info.height);
    assert!(witness.next_frame().unwrap().unwrap().coding.is_none());
    let mut decoder = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Owned(file), &[])
        .unwrap();
    for (i, expected) in images.iter().enumerate() {
        let frame = decoder.render_next_frame(None).unwrap().unwrap();
        assert_pixels(frame.pixels(), &info, expected);
        assert_eq!(frame.frame_index(), i as u32);
        assert_eq!(frame.duration_ms(), 33);
    }
    assert!(decoder.render_next_frame(None).unwrap().is_none());
}

#[derive(Default)]
struct Sink {
    bytes: Vec<u8>,
    stride: usize,
    width: u32,
    rows: u32,
    next: u32,
    begins: u32,
    finishes: u32,
    fail: Option<&'static str>,
}
impl DecodeRowSink for Sink {
    fn begin(
        &mut self,
        w: u32,
        h: u32,
        d: PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
        self.begins += 1;
        if self.fail == Some("begin") {
            return Err(Box::new(std::io::Error::other("begin")));
        }
        self.width = w;
        self.rows = h;
        self.next = 0;
        self.stride = (w as usize + 3) * d.bytes_per_pixel();
        self.bytes = vec![0xee; self.stride * h as usize];
        Ok(())
    }
    fn provide_next_buffer(
        &mut self,
        y: u32,
        h: u32,
        w: u32,
        d: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, zencodec::decode::SinkError> {
        assert_eq!(y, self.next);
        assert_eq!(w, self.width);
        assert!(h > 0 && y + h <= self.rows);
        if self.fail == Some("buffer") {
            return Err(Box::new(std::io::Error::other("buffer")));
        }
        self.next += h;
        Ok(PixelSliceMut::new(
            &mut self.bytes[y as usize * self.stride..(y + h) as usize * self.stride],
            w,
            h,
            self.stride,
            d,
        )
        .unwrap())
    }
    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        if self.fail == Some("finish") {
            return Err(Box::new(std::io::Error::other("finish")));
        }
        assert_eq!(self.next, self.rows);
        self.finishes += 1;
        Ok(())
    }
}

#[test]
fn sink_accepts_padding_and_preserves_lifecycle_errors() {
    let (file, info, images) = unified(SampleType::U16);
    let mut decoder = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Borrowed(&file), &[])
        .unwrap();
    let mut sink = Sink::default();
    for expected in &images {
        assert!(
            decoder
                .render_next_frame_to_sink(None, &mut sink)
                .unwrap()
                .is_some()
        );
        for y in 0..info.height as usize {
            assert_eq!(
                &sink.bytes[y * sink.stride..y * sink.stride + info.row_bytes()],
                &expected[y * info.row_bytes()..(y + 1) * info.row_bytes()]
            );
            assert!(
                sink.bytes[y * sink.stride + info.row_bytes()..(y + 1) * sink.stride]
                    .iter()
                    .all(|&b| b == 0xee)
            );
        }
    }
    assert_eq!((sink.begins, sink.finishes), (3, 3));
    assert!(
        decoder
            .render_next_frame_to_sink(None, &mut sink)
            .unwrap()
            .is_none()
    );
    assert_eq!((sink.begins, sink.finishes), (3, 3));
    for stage in ["begin", "buffer", "finish"] {
        let mut decoder = DecodeConfig::default()
            .job()
            .animation_frame_decoder(Cow::Borrowed(&file), &[])
            .unwrap();
        let mut sink = Sink {
            fail: Some(stage),
            ..Default::default()
        };
        let error = decoder
            .render_next_frame_to_sink(None, &mut sink)
            .err()
            .unwrap();
        match error {
            DecodeError::Sink(e) => assert_eq!(
                e.downcast_ref::<std::io::Error>().unwrap().to_string(),
                stage
            ),
            e => panic!("{e}"),
        }
        assert_eq!(sink.finishes, 0);
        assert!(decoder.render_next_frame(None).is_err());
    }
}

#[derive(Clone)]
struct Flag(Arc<AtomicBool>);
impl zencodec::enough::Stop for Flag {
    fn check(&self) -> Result<(), zencodec::enough::StopReason> {
        if self.0.load(Ordering::SeqCst) {
            Err(zencodec::enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn job_and_per_call_cancellation_are_sticky() {
    let (file, _, _) = unified(SampleType::U8);
    for per_call in [false, true] {
        let flag = Flag(Arc::new(AtomicBool::new(false)));
        let job = DecodeConfig::default().job();
        let job = if per_call {
            job
        } else {
            job.with_stop(StopToken::new(flag.clone()))
        };
        let mut decoder = job
            .animation_frame_decoder(Cow::Borrowed(&file), &[])
            .unwrap();
        assert!(decoder.render_next_frame(None).unwrap().is_some());
        flag.0.store(true, Ordering::SeqCst);
        let call = how_far::StopOnly::borrowed(&flag);
        let stop = per_call.then_some(&call as &dyn zencodec::Pulse);
        assert!(matches!(
            decoder.render_next_frame(stop),
            Err(DecodeError::Codec(Error::Cancelled))
        ));
        flag.0.store(false, Ordering::SeqCst);
        let mut sink = Sink::default();
        assert!(decoder.render_next_frame_to_sink(None, &mut sink).is_err());
        assert_eq!((sink.begins, sink.finishes), (0, 0));
    }
}

#[test]
fn limits_include_owned_input_sequence_totals_and_rounded_timeline() {
    let (file, info, _) = unified(SampleType::U8);
    for limits in [
        ResourceLimits::none().with_max_input_bytes(file.len() as u64 - 1),
        ResourceLimits::none().with_max_frames(2),
        ResourceLimits::none()
            .with_max_total_pixels(u64::from(info.width) * u64::from(info.height) * 3 - 1),
        ResourceLimits::none().with_max_animation_ms(33),
        ResourceLimits::none().with_max_animation_ms(34),
        ResourceLimits::none().with_max_memory(1),
        ResourceLimits::none()
            .with_max_memory(file.len() as u64 + info.checked_buffer_len().unwrap() as u64 - 1),
    ] {
        match DecodeConfig::default()
            .job()
            .with_limits(limits)
            .animation_frame_decoder(Cow::Borrowed(&file), &[])
        {
            Err(e) => assert!(matches!(e, DecodeError::Codec(Error::TooLarge))),
            Ok(mut decoder) => {
                let error = loop {
                    match decoder.render_next_frame(None) {
                        Ok(Some(_)) => {}
                        Ok(None) => panic!("over-budget animation succeeded"),
                        Err(e) => break e,
                    }
                };
                assert!(matches!(error, DecodeError::Codec(Error::TooLarge)));
                assert!(decoder.render_next_frame(None).is_err());
            }
        }
    }
    let limits = ResourceLimits::none()
        .with_max_input_bytes(file.len() as u64)
        .with_max_frames(3)
        .with_max_total_pixels(u64::from(info.width) * u64::from(info.height) * 3)
        .with_max_animation_ms(35)
        .with_max_memory(64 << 20);
    let mut decoder = DecodeConfig::default()
        .job()
        .with_limits(limits)
        .animation_frame_decoder(Cow::Owned(file.clone()), &[])
        .unwrap();
    for _ in 0..3 {
        assert!(decoder.render_next_frame(None).unwrap().is_some());
    }
    assert!(decoder.render_next_frame(None).unwrap().is_none());
    let policy = imij::decode_resources::DecodePolicy::default().with_max_work_units(0);
    match DecodeConfig::default()
        .with_admission(policy)
        .job()
        .animation_frame_decoder(Cow::Owned(file), &[])
    {
        Err(e) => assert!(matches!(e, DecodeError::Codec(Error::TooLarge))),
        Ok(mut decoder) => assert!(matches!(
            decoder.render_next_frame(None),
            Err(DecodeError::Codec(Error::TooLarge))
        )),
    }
}

#[test]
fn admitted_work_is_cumulative_between_frames() {
    let (file, _, _) = unified(SampleType::U8);
    let mut witness = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Borrowed(&file), &[])
        .unwrap();
    assert!(witness.render_next_frame(None).unwrap().is_some());
    let first_work = witness.usage().work_units;
    assert!(first_work > 0);
    assert!(witness.render_next_frame(None).unwrap().is_some());
    assert!(witness.usage().work_units > first_work);
    let policy = imij::decode_resources::DecodePolicy::default().with_max_work_units(first_work);
    let mut decoder = DecodeConfig::default()
        .with_admission(policy)
        .job()
        .animation_frame_decoder(Cow::Borrowed(&file), &[])
        .unwrap();
    assert!(decoder.render_next_frame(None).unwrap().is_some());
    assert!(matches!(
        decoder.render_next_frame(None),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
    assert!(decoder.render_next_frame(None).is_err());
}

#[test]
// Also checks zencodec's deprecated millisecond accessor, which callers still use.
#[allow(deprecated)]
fn still_is_one_frame_and_pages_are_explicitly_unsupported() {
    let info = ImageInfo::new(7, 5, Channels::Rgb, SampleType::U8);
    let raw = pixels(&info, 71);
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &metadata(0),
        )
        .unwrap();
    let mut decoder = DecodeConfig::default()
        .job()
        .animation_frame_decoder(Cow::Owned(file), &[])
        .unwrap();
    assert_eq!(decoder.frame_count(), Some(1));
    let frame = decoder.render_next_frame(None).unwrap().unwrap();
    assert_pixels(frame.pixels(), &info, &raw);
    assert_eq!(frame.duration_ms(), 0);
    assert!(decoder.render_next_frame(None).unwrap().is_none());
    let mut legacy = FrameEncoder::pages(info.clone(), metadata(0), config());
    legacy.push_page(&raw, info.row_bytes(), &info).unwrap();
    let mut unified = SequenceEncoder::new(SequenceKind::Pages, config());
    unified
        .push(SequenceItem::new(&raw, info.row_bytes(), &info))
        .unwrap();
    for file in [legacy.finish().unwrap(), unified.finish().unwrap()] {
        assert!(matches!(
            DecodeConfig::default()
                .job()
                .animation_frame_decoder(Cow::Owned(file), &[]),
            Err(DecodeError::Codec(Error::Unsupported(_)))
        ));
    }
}

#[test]
// Also checks zencodec's deprecated millisecond accessor, which callers still use.
#[allow(deprecated)]
fn common_timing_preserves_full_native_u64_range() {
    let info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U8);
    let raw = [1, 2, 3];
    for duration in [
        FrameDuration {
            num: u64::from(u32::MAX),
            den: 1000,
        },
        FrameDuration {
            num: u64::from(u32::MAX) + 1,
            den: 1000,
        },
        FrameDuration {
            num: u64::MAX / 1000 + 1,
            den: 1,
        },
        FrameDuration {
            num: u64::MAX,
            den: u32::MAX,
        },
        FrameDuration {
            num: u64::MAX,
            den: 1,
        },
    ] {
        let mut encoder = SequenceEncoder::new(SequenceKind::Animation, config());
        encoder
            .push(SequenceItem::new(&raw, 3, &info).with_duration(duration))
            .unwrap();
        let file = encoder.finish().unwrap();
        let mut decoder = DecodeConfig::default()
            .job()
            .with_limits(ResourceLimits::none())
            .animation_frame_decoder(Cow::Borrowed(&file), &[])
            .unwrap();
        let frame = decoder.render_next_frame_owned(None).unwrap().unwrap();
        assert_eq!(frame.duration_ms(), u32::MAX);
        assert_eq!(
            frame.duration(),
            zencodec::animation::FrameDuration::new(duration.num, duration.den).unwrap()
        );
        assert_eq!(
            frame.extras::<DecodedFrameMetadata>().unwrap().duration,
            duration
        );
        assert!(matches!(
            DecodeConfig::default()
                .job()
                .with_limits(ResourceLimits::none().with_max_animation_ms(1))
                .animation_frame_decoder(Cow::Borrowed(&file), &[]),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
    }
}
