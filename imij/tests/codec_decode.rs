use imij::ImiEncoderConfig;
use imij::codec::{DecodeConfig, DecodeError, DecodedMetadata};
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};
use std::borrow::Cow;
use zencodec::decode::{Decode, DecodeJob, DecodeRowSink, DecoderConfig, StreamingDecode};
use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
use zencodec::{ResourceLimits, StopToken};
use zenpixels::{PixelDescriptor, PixelSlice, PixelSliceMut};

fn file(info: &ImageInfo, meta: &Metadata) -> (Vec<u8>, Vec<u8>) {
    let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 71 + i / 17) as u8)
        .collect();
    let enc = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(0.0);
        cfg
    }
    .encode_samples(imij::ImageRef::new(&raw, info.row_bytes(), info), meta)
    .unwrap();
    (enc, raw)
}
struct Sink {
    bytes: Vec<u8>,
    stride: usize,
    width: u32,
    rows: u32,
    count: u32,
    descriptor: PixelDescriptor,
    finished: bool,
    bad: bool,
    fail: bool,
    fail_begin: bool,
    fail_finish: bool,
}
impl Default for Sink {
    fn default() -> Self {
        Self {
            bytes: vec![],
            stride: 0,
            width: 0,
            rows: 0,
            count: 0,
            descriptor: PixelDescriptor::RGB8_SRGB,
            finished: false,
            bad: false,
            fail: false,
            fail_begin: false,
            fail_finish: false,
        }
    }
}
impl DecodeRowSink for Sink {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
        if self.fail_begin {
            return Err(Box::new(std::io::Error::other("begin witness")));
        }
        self.stride = (width as usize + 2) * descriptor.bytes_per_pixel();
        self.bytes = vec![0xee; self.stride * height as usize];
        self.width = width;
        self.rows = height;
        self.descriptor = descriptor;
        Ok(())
    }
    fn provide_next_buffer(
        &mut self,
        y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, zencodec::decode::SinkError> {
        assert_eq!(y, self.count);
        assert_eq!(height, 1);
        assert_eq!(width, self.width);
        assert_eq!(descriptor, self.descriptor);
        if self.fail {
            return Err(Box::new(std::io::Error::other("sink witness")));
        }
        self.count += 1;
        let width = if self.bad { width - 1 } else { width };
        Ok(PixelSliceMut::new(
            &mut self.bytes[y as usize * self.stride..(y + 1) as usize * self.stride],
            width,
            1,
            self.stride,
            descriptor,
        )
        .unwrap())
    }
    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        if self.fail_finish {
            return Err(Box::new(std::io::Error::other("finish witness")));
        }
        self.finished = true;
        Ok(())
    }
}
#[test]
fn all_native_baseline_descriptors_owned_borrowed_and_padded_sink() {
    for &desc in ImiEncoderConfig::supported_descriptors() {
        let stride = 13 * desc.bytes_per_pixel();
        let raw: Vec<u8> = (0..stride * 7).map(|i| (i * 37 + i / 7) as u8).collect();
        let enc = imij::ImiEncoderConfig::default()
            .with_generic_effort(0)
            .job()
            .encoder()
            .unwrap()
            .encode(PixelSlice::new(&raw, 13, 7, stride, desc).unwrap())
            .unwrap();
        for data in [Cow::Borrowed(enc.data()), Cow::Owned(enc.data().to_vec())] {
            let mut pull = DecodeConfig::default()
                .job()
                .with_limits(ResourceLimits::none().with_max_memory(16 << 20))
                .streaming_decoder(data.clone(), &[])
                .unwrap();
            assert_eq!(pull.info().width, 13);
            assert_eq!(
                pull.output()
                    .extras::<DecodedMetadata>()
                    .unwrap()
                    .image
                    .width,
                13
            );
            let (y, batch) = pull.next_batch().unwrap().unwrap();
            assert_eq!((y, batch.width(), batch.rows()), (0, 13, 7));
            assert_eq!(batch.descriptor(), desc);
            for row in 0..7 {
                assert_eq!(
                    batch.row(row),
                    &raw[row as usize * stride..(row as usize + 1) * stride]
                );
            }
            assert!(pull.next_batch().unwrap().is_none());
            assert!(pull.next_batch().unwrap().is_none());
            let out = DecodeConfig::default()
                .job()
                .with_limits(ResourceLimits::none().with_max_memory(16 << 20))
                .decoder(data, &[])
                .unwrap()
                .decode()
                .unwrap();
            assert_eq!(out.pixels().descriptor(), desc);
            for y in 0..7 {
                assert_eq!(
                    out.pixels().row(y),
                    &raw[y as usize * stride..(y as usize + 1) * stride]
                );
            }
            assert_eq!(out.extras::<DecodedMetadata>().unwrap().image.width, 13);
        }
        let mut sink = Sink::default();
        DecodeConfig::default()
            .job()
            .push_decoder(Cow::Borrowed(enc.data()), &mut sink, &[])
            .unwrap();
        assert!(sink.finished);
        assert_eq!(sink.count, 7);
        for y in 0..7 {
            assert_eq!(
                &sink.bytes[y * sink.stride..y * sink.stride + stride],
                &raw[y * stride..(y + 1) * stride]
            );
            assert!(
                sink.bytes[y * sink.stride + stride..(y + 1) * sink.stride]
                    .iter()
                    .all(|&b| b == 0xee)
            );
        }
    }
}
#[test]
fn lossy_and_native_metadata_are_not_reinterpreted() {
    let mut info = ImageInfo::new(19, 11, Channels::Rgba, SampleType::U16)
        .with_significant_bits(&[10; 4])
        .unwrap();
    info.alpha_mode = imij::AlphaMode::Premultiplied;
    let mut cll = [0; 8];
    cll[..4].copy_from_slice(&1_234_567u32.to_be_bytes());
    cll[4..].copy_from_slice(&u32::MAX.to_be_bytes());
    let meta = Metadata {
        orientation: 6,
        content_light_level: Some(cll),
        exif: Some(b"Exif\0\0".to_vec()),
        xmp: Some(b"<x>value</x>".to_vec()),
        ..Default::default()
    };
    let (enc, raw) = file(&info, &meta);
    let out = DecodeConfig::default()
        .job()
        .decoder(Cow::Borrowed(&enc), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(out.extras::<DecodedMetadata>().unwrap().image, info);
    assert_eq!(out.extras::<DecodedMetadata>().unwrap().metadata, meta);
    assert_eq!(out.info().orientation as u8, 6);
    assert_eq!(out.info().source_color.content_light_level, None);
    assert!(!out.info().warnings.is_empty());
    assert_eq!(
        out.pixels().descriptor().alpha,
        Some(zenpixels::AlphaMode::Premultiplied)
    );
    for y in 0..11 {
        assert_eq!(
            out.pixels().row(y),
            &raw[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()]
        );
    }
    let rgb = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
    let (_, raw) = file(&rgb, &Metadata::default());
    let enc = imij::ImiEncoderConfig::lossy(60.0)
        .with_effort(0.0)
        .encode_samples(
            imij::ImageRef::new(&raw, rgb.row_bytes(), &rgb),
            &Metadata::default(),
        )
        .unwrap();
    let expected = imij::decode(&enc).unwrap().1;
    let mut sink = Sink::default();
    DecodeConfig::default()
        .job()
        .push_decoder(Cow::Borrowed(&enc), &mut sink, &[])
        .unwrap();
    for y in 0..17 {
        assert_eq!(
            &sink.bytes[y * sink.stride..y * sink.stride + 99],
            &expected[y * 99..(y + 1) * 99]
        );
    }
}
#[test]
// Also checks the legacy job `with_policy`, deprecated in zencodec 0.2; imij enforces it.
#[allow(deprecated)]
fn metadata_policy_and_compressed_expansion_admission() {
    let info = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U8);
    let meta = Metadata {
        exif: Some(vec![42; 10000]),
        xmp: Some(vec![43; 9000]),
        ..Default::default()
    };
    let (enc, _) = file(&info, &meta);
    let enc = imij::compress_metadata(&enc, 1 << 20).unwrap();
    assert!(enc.len() < 19000);
    let job = DecodeConfig::default()
        .with_admission(
            imij::decode_resources::DecodePolicy::default().with_max_metadata_bytes(100),
        )
        .job();
    assert!(matches!(
        job.probe_full(&enc),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
    let out = DecodeConfig::default()
        .job()
        .with_policy(zencodec::decode::DecodePolicy::strict())
        .decoder(Cow::Borrowed(&enc), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert!(out.info().embedded_metadata.is_empty());
    let native = out.extras::<DecodedMetadata>().unwrap();
    assert!(native.metadata.exif.is_none() && native.metadata.xmp.is_none());
    let (icc, _) = file(
        &info,
        &Metadata {
            icc: Some(vec![0; 128]),
            ..Default::default()
        },
    );
    let out = DecodeConfig::default()
        .job()
        .decoder(Cow::Borrowed(&icc), &[])
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(
        out.info().source_color.color_authority,
        zencodec::ColorAuthority::Icc
    );
    assert_eq!(
        out.pixels().descriptor().transfer,
        zenpixels::TransferFunction::Unknown
    );
    assert!(out.pixels().color_context().unwrap().cicp.is_none());
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_policy(zencodec::decode::DecodePolicy::strict())
            .probe_full(&icc),
        Err(DecodeError::Codec(Error::Unsupported(_)))
    ));
}
#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn limits_cancellation_and_sink_errors() {
    let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
    let (enc, _) = file(&info, &Metadata::default());
    for limits in [
        ResourceLimits::none().with_max_pixels(560),
        ResourceLimits::none().with_max_width(32),
        ResourceLimits::none().with_max_input_bytes(enc.len() as u64 - 1),
        ResourceLimits::none().with_max_total_pixels(560),
        ResourceLimits::none().with_max_frames(0),
        ResourceLimits::none().with_max_memory(1),
    ] {
        let mut sink = Sink::default();
        assert!(matches!(
            DecodeConfig::default()
                .job()
                .with_limits(limits)
                .push_decoder(Cow::Borrowed(&enc), &mut sink, &[]),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        assert_eq!(sink.count, 0);
        assert!(!sink.finished);
    }
    let mut bad = Sink {
        bad: true,
        ..Default::default()
    };
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .push_decoder(Cow::Borrowed(&enc), &mut bad, &[]),
        Err(DecodeError::Codec(Error::InvalidInput(_)))
    ));
    assert!(!bad.finished);
    assert!(bad.bytes.iter().all(|&b| b == 0xee));
    let mut fail = Sink {
        fail: true,
        ..Default::default()
    };
    let e = DecodeConfig::default()
        .job()
        .push_decoder(Cow::Borrowed(&enc), &mut fail, &[])
        .unwrap_err();
    match e {
        DecodeError::Sink(e) => assert_eq!(
            e.downcast_ref::<std::io::Error>().unwrap().to_string(),
            "sink witness"
        ),
        e => panic!("{e}"),
    }
    assert!(!fail.finished);
    #[derive(Clone)]
    struct Cancel;
    impl zencodec::enough::Stop for Cancel {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            Err(zencodec::enough::StopReason::Cancelled)
        }
    }
    let mut sink = Sink::default();
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_stop(StopToken::new(Cancel))
            .push_decoder(Cow::Borrowed(&enc), &mut sink, &[]),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert!(sink.bytes.is_empty());
}
#[test]
fn primary_page_decode_reports_full_sequence_without_returning_later_pixels() {
    let info = ImageInfo::new(7, 5, Channels::Rgb, SampleType::U8);
    let (_, raw) = file(&info, &Metadata::default());
    let mut pages = imij::FrameEncoder::pages(
        info.clone(),
        Metadata::default(),
        imij::FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    pages.push_page(&raw, info.row_bytes(), &info).unwrap();
    pages
        .push_page(&vec![0; raw.len()], info.row_bytes(), &info)
        .unwrap();
    let enc = pages.finish().unwrap();
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_limits(ResourceLimits::none().with_max_frames(1))
            .probe_full(&enc),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
    let job = DecodeConfig::default().job();
    assert!(job.probe(&enc).unwrap().source_color.cicp.is_none());
    assert!(matches!(
        job.probe_full(&enc).unwrap().sequence,
        zencodec::ImageSequence::Multi {
            image_count: Some(2),
            ..
        }
    ));
    let out = job.decoder(Cow::Owned(enc), &[]).unwrap().decode().unwrap();
    for y in 0..5 {
        assert_eq!(
            out.pixels().row(y),
            &raw[y as usize * 21..(y as usize + 1) * 21]
        );
    }
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn cancellation_during_rows_does_not_finish_or_write_the_cancelled_row() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
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
    struct CancelSink {
        flag: Flag,
        sink: Sink,
    }
    impl DecodeRowSink for CancelSink {
        fn begin(
            &mut self,
            w: u32,
            h: u32,
            d: PixelDescriptor,
        ) -> Result<(), zencodec::decode::SinkError> {
            self.sink.begin(w, h, d)
        }
        fn provide_next_buffer(
            &mut self,
            y: u32,
            h: u32,
            w: u32,
            d: PixelDescriptor,
        ) -> Result<PixelSliceMut<'_>, zencodec::decode::SinkError> {
            if y == 1 {
                self.flag.0.store(true, Ordering::SeqCst);
            }
            self.sink.provide_next_buffer(y, h, w, d)
        }
        fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
            self.sink.finish()
        }
    }
    let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
    let (enc, raw) = file(&info, &Metadata::default());
    let flag = Flag(Arc::new(AtomicBool::new(false)));
    let mut sink = CancelSink {
        flag: flag.clone(),
        sink: Sink::default(),
    };
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_stop(StopToken::new(flag))
            .push_decoder(Cow::Borrowed(&enc), &mut sink, &[]),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert_eq!(sink.sink.count, 2);
    assert!(!sink.sink.finished);
    assert_eq!(&sink.sink.bytes[..99], &raw[..99]);
    assert!(
        sink.sink.bytes[sink.sink.stride..]
            .iter()
            .all(|&b| b == 0xee)
    );
}

#[test]
fn float_payloads_and_exact_limit_boundaries() {
    for (sample, patterns) in [
        (
            SampleType::F16,
            [0u32, 0x8000, 1, 0x3c00, 0x7c00, 0xfc00, 0x7c01, 0x7eab],
        ),
        (
            SampleType::F32,
            [
                0, 0x80000000, 1, 0x3f800000, 0x7f800000, 0xff800000, 0x7f800001, 0x7fc12abc,
            ],
        ),
    ] {
        let info = ImageInfo::new(8, 2, Channels::Rgba, sample);
        let mut raw = Vec::new();
        for i in 0..64 {
            if sample == SampleType::F16 {
                raw.extend_from_slice(&(patterns[i % 8] as u16).to_ne_bytes());
            } else {
                raw.extend_from_slice(&patterns[i % 8].to_ne_bytes());
            }
        }
        let enc = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&raw, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let limits = ResourceLimits::none()
            .with_max_pixels(16)
            .with_max_width(8)
            .with_max_height(2)
            .with_max_total_pixels(16)
            .with_max_frames(1)
            .with_max_input_bytes(enc.len() as u64)
            .with_max_memory(16 << 20);
        let out = DecodeConfig::default()
            .job()
            .with_limits(limits)
            .decoder(Cow::Borrowed(&enc), &[])
            .unwrap()
            .decode()
            .unwrap();
        for y in 0..2 {
            assert_eq!(
                out.pixels().row(y),
                &raw[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()]
            );
        }
        assert!(matches!(
            DecodeConfig::default()
                .job()
                .with_limits(ResourceLimits::none().with_max_height(1))
                .probe(&enc),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
    }
}

#[test]
fn sink_lifecycle_errors_and_reconstruction_cap_precede_begin() {
    let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
    let (enc, _) = file(&info, &Metadata::default());
    for (begin, finish, message) in [
        (true, false, "begin witness"),
        (false, true, "finish witness"),
    ] {
        let mut sink = Sink {
            fail_begin: begin,
            fail_finish: finish,
            ..Default::default()
        };
        let err = DecodeConfig::default()
            .job()
            .push_decoder(Cow::Borrowed(&enc), &mut sink, &[])
            .unwrap_err();
        match err {
            DecodeError::Sink(e) => assert_eq!(
                e.downcast_ref::<std::io::Error>().unwrap().to_string(),
                message
            ),
            e => panic!("{e}"),
        }
        assert!(!sink.finished);
        assert_eq!(sink.count, if begin { 0 } else { 17 });
    }
    // The full container fits; model/working buffers do not. No sink begin.
    let limits = ResourceLimits::none().with_max_memory(64 << 10);
    assert!(
        DecodeConfig::default()
            .job()
            .with_limits(limits)
            .probe_full(&enc)
            .is_ok()
    );
    let mut sink = Sink::default();
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_limits(limits)
            .push_decoder(Cow::Borrowed(&enc), &mut sink, &[]),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
    assert!(sink.bytes.is_empty());
    assert_eq!(sink.count, 0);
    assert!(!sink.finished);
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn buffered_pull_limits_and_cancellation_precede_batch_delivery() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
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
    let info = ImageInfo::new(19, 13, Channels::Rgba, SampleType::U16);
    let (encoded, _) = file(&info, &Metadata::default());
    for limits in [
        ResourceLimits::none().with_max_memory(1),
        ResourceLimits::none().with_max_input_bytes(1),
    ] {
        assert!(matches!(
            DecodeConfig::default()
                .job()
                .with_limits(limits)
                .streaming_decoder(Cow::Borrowed(&encoded), &[]),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
    }
    let flag = Flag(Arc::new(AtomicBool::new(false)));
    let mut pull = DecodeConfig::default()
        .job()
        .with_stop(StopToken::new(flag.clone()))
        .streaming_decoder(Cow::Owned(encoded), &[])
        .unwrap();
    flag.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        pull.next_batch(),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert!(matches!(
        pull.next_batch(),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert_eq!(pull.info().width, 19);
    assert!(DecodeConfig::capabilities().streaming());
    fn assert_send_static<T: Send + 'static>(_: &T) {}
    assert_send_static(&pull);
}

#[test]
fn diffuse_white_native_absence_and_shared_hdr_convention_are_distinct() {
    for transfer in [1, 13, 16, 18, 2] {
        for icc in [None, Some(vec![0; 128])] {
            for explicit in [None, Some(zenpixels::hdr::DiffuseWhite::new(187.25))] {
                let mut info = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U8);
                info.cicp.transfer_characteristics = transfer;
                let meta = Metadata {
                    diffuse_white: explicit,
                    icc: icc.clone(),
                    ..Default::default()
                };
                let expected = explicit.or_else(|| {
                    (icc.is_none() && matches!(transfer, 16 | 18))
                        .then_some(zenpixels::hdr::DiffuseWhite::BT2408)
                });
                let (enc, raw) = file(&info, &meta);
                let job = DecodeConfig::default().job();
                assert_eq!(
                    job.probe_full(&enc).unwrap().source_color.diffuse_white,
                    expected
                );
                let out = job
                    .decoder(Cow::Borrowed(&enc), &[])
                    .unwrap()
                    .decode()
                    .unwrap();
                assert_eq!(out.info().source_color.diffuse_white, expected);
                assert_eq!(
                    out.extras::<DecodedMetadata>()
                        .unwrap()
                        .metadata
                        .diffuse_white,
                    explicit
                );
                assert_eq!(
                    out.pixels().color_context().unwrap().diffuse_white,
                    expected
                );
                for y in 0..2 {
                    assert_eq!(
                        out.pixels().row(y),
                        &raw[y as usize * 9..(y as usize + 1) * 9]
                    );
                }
            }
        }
    }
}
