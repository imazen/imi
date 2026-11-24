use super::*;
use crate::codec::DecodeConfig;
use crate::components::{
    ComponentEncodeConfig, ComponentEncodeLimits, ComponentSource, encode_groups,
};
use crate::stream_encode::{GroupRect, StreamEncodeError};
use crate::{ImiEncoderConfig, LossyColor, LossyEntropy, LossyQuality, RowGroups};
use std::io::Cursor;
use zencodec::decode::{DecodeJob, DecoderConfig};
use zenpixels::plane::{AnyPlane, Plane, PlaneEncoding, PlaneRole, PlaneSpec};
use zenpixels::sample::{ChromaSiting, ChromaSubsampling, SampleEncoding};
use zenpixels::{ChannelType, Cicp, ColorAuthority};

struct Source {
    planes: Vec<PixelStorage>,
    strides: Vec<usize>,
}
impl ComponentSource for Source {
    fn group(&mut self, component: u8, rect: GroupRect) -> Result<AnyPlane<'_>, StreamEncodeError> {
        let index = usize::from(component);
        let stride = self.strides[index];
        let start = rect.y as usize * stride + rect.x as usize;
        Ok(match &self.planes[index] {
            PixelStorage::U8(v) => AnyPlane::U8(
                Plane::new(
                    &v[start..],
                    rect.width as usize,
                    rect.height as usize,
                    stride,
                )
                .unwrap(),
            ),
            PixelStorage::U16(v) => AnyPlane::U16(
                Plane::new(
                    &v[start..],
                    rect.width as usize,
                    rect.height as usize,
                    stride,
                )
                .unwrap(),
            ),
            _ => unreachable!(),
        })
    }
}
fn fixture(
    bits: u8,
    sampling: ChromaSubsampling,
    entropy: Option<LossyEntropy>,
    icc: bool,
) -> (Vec<u8>, [Vec<u16>; 4]) {
    let (width, height) = (37, 35);
    let specs = [
        PlaneRole::Luma,
        PlaneRole::Cb,
        PlaneRole::Cr,
        PlaneRole::Alpha,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, role)| {
        let depth = if i == 3 { 16 } else { bits };
        PlaneSpec::new(
            role,
            PlaneEncoding::Code(
                SampleEncoding::new(
                    if depth == 8 {
                        ChannelType::U8
                    } else {
                        ChannelType::U16
                    },
                    depth,
                    0,
                )
                .unwrap(),
            ),
        )
    })
    .collect();
    let layout = PlanarLayout::new(specs, sampling, ChromaSiting::Left).unwrap();
    let mut source = Source {
        planes: Vec::new(),
        strides: Vec::new(),
    };
    let mut expected: [Vec<u16>; 4] = core::array::from_fn(|_| Vec::new());
    for (i, out) in expected.iter_mut().enumerate() {
        let (w, h) = layout.plane_size(i, width, height).unwrap();
        let depth = if i == 3 { 16 } else { bits };
        let max = (1u32 << depth) - 1;
        let stride = w as usize + 3;
        let mut samples = vec![0xd1u16; stride * (h as usize - 1) + w as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let value = match (x, y) {
                    (0, 0) => 0,
                    (1, 0) => max,
                    _ => ((x * 107 + y * 151 + i * 631) ^ (x * y * 49)) as u32 & max,
                } as u16;
                samples[y * stride + x] = value;
                out.push(value);
            }
        }
        source.strides.push(stride);
        source.planes.push(if depth == 8 {
            PixelStorage::U8(samples.into_iter().map(|v| v as u8).collect())
        } else {
            PixelStorage::U16(samples)
        });
    }
    let encoders: Vec<_> = (0..4)
        .map(|i| {
            if let Some(entropy) = entropy.filter(|_| i != 3) {
                ImiEncoderConfig::lossy(LossyQuality::Ssim2(80.0))
                    .with_effort(0.0)
                    .with_column_groups(16)
                    .tune_lossy(|t| {
                        t.aq_gamma = 0.0;
                        t.color = LossyColor::YCoCg;
                        t.stripe_block_rows = 1;
                        t.entropy = entropy;
                    })
            } else {
                ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(16, 8, 64 << 20))
            }
        })
        .collect();
    let mut cfg = ComponentEncodeConfig::new(
        width,
        height,
        &layout,
        Cicp::new(9, 16, 9, false),
        &encoders,
        ComponentEncodeLimits::new(192 << 20, 8 << 20, 1 << 20, 1 << 20, 4 << 20),
    );
    if icc {
        // The native contract retains opaque ICC bytes and performs no CMS.
        cfg.icc = Some(b"opaque native feed profile");
        cfg.color_authority = ColorAuthority::Icc;
    }
    let mut numeric: [Cursor<Vec<u8>>; 4] = core::array::from_fn(|_| Cursor::new(Vec::new()));
    let (encoded, _, _) = encode_groups(
        &cfg,
        &mut source,
        Vec::new(),
        &mut numeric,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        &how_far::Unstoppable,
    )
    .unwrap();
    (encoded, expected)
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Observed {
    rows: [Vec<u16>; 4],
    row_counts: [u32; 4],
    groups: Vec<(u32, u32)>,
    header: Option<(u32, u32, ColorAuthority, Option<Vec<u8>>)>,
    complete: Option<(bool, bool)>,
    last_pointer: usize,
}
impl Observed {
    fn event(&mut self, event: ComponentEvent<'_>) -> Result<bool, Error> {
        match event {
            ComponentEvent::Declaration(h) => {
                assert!(self.header.is_none());
                self.header = Some((
                    h.width,
                    h.height,
                    h.color_authority,
                    h.icc.as_deref().map(Vec::from),
                ));
            }
            ComponentEvent::Row {
                component,
                y,
                samples,
            } => {
                let i = usize::from(component);
                assert_eq!(y, self.row_counts[i]);
                self.row_counts[i] += 1;
                match samples {
                    AnyPlane::U8(p) => {
                        let row = p.row(0).unwrap();
                        self.last_pointer = row.as_ptr() as usize;
                        self.rows[i].extend(row.iter().map(|&v| u16::from(v)));
                    }
                    AnyPlane::U16(p) => {
                        let row = p.row(0).unwrap();
                        self.last_pointer = row.as_ptr() as usize;
                        self.rows[i].extend_from_slice(row);
                    }
                    _ => unreachable!(),
                }
                return Ok(true);
            }
            ComponentEvent::Group { luma_top, luma_end } => self.groups.push((luma_top, luma_end)),
            ComponentEvent::Complete {
                encoded_integrity_verified,
                decoded_attestation,
            } => {
                assert!(self.complete.is_none());
                self.complete = Some((encoded_integrity_verified, decoded_attestation));
            }
        }
        Ok(false)
    }
}

fn pump(
    decoder: &mut ComponentFeedDecoder,
    input: &[u8],
    at: &mut usize,
    out: &mut Observed,
    stop_at_row: bool,
) -> Result<ComponentFeedStatus, DecodeError> {
    let bound = input.len() * 10 + 4096;
    for _ in 0..bound {
        match decoder.advance()? {
            ComponentFeedStatus::NeedInput => {
                let request = decoder.input_request().expect("pending native read");
                assert_eq!(request.offset(), Some(*at as u64));
                let zencodec::decode::InputRequest::Range(range) = request else {
                    panic!("expected a bounded sequential read span");
                };
                assert!(range.length != 0);
                if *at == input.len() {
                    return Ok(ComponentFeedStatus::NeedInput);
                }
                let end = input.len().min(*at + 11);
                let accepted = decoder.push(&input[*at..end])?;
                assert!(accepted != 0, "input wait with a full unconsumed queue");
                *at += accepted;
            }
            ComponentFeedStatus::Event => {
                if out.event(decoder.event()?.unwrap())? && stop_at_row {
                    return Ok(ComponentFeedStatus::Event);
                }
            }
            ComponentFeedStatus::Complete => {
                out.event(decoder.event()?.unwrap())?;
                return Ok(ComponentFeedStatus::Complete);
            }
        }
    }
    panic!("caller-fed component execution made no bounded progress")
}

#[test]
fn tiny_queues_preserve_native_bits_groups_eof_and_single_input_charge() {
    for bits in [8, 10, 12, 16] {
        for sampling in [
            ChromaSubsampling::Yuv444,
            ChromaSubsampling::Yuv422,
            ChromaSubsampling::Yuv420,
        ] {
            for entropy in [
                None,
                Some(LossyEntropy::RunLevel),
                Some(LossyEntropy::SplitHuf),
            ] {
                let (encoded, source) = fixture(bits, sampling, entropy, false);
                let mut blocking = Observed::default();
                crate::components::decode_reader(&encoded[..], DecodePolicy::default(), |event| {
                    blocking.event(event).map(|_| ())
                })
                .unwrap();
                if entropy.is_none() {
                    assert_eq!(blocking.rows, source);
                }
                assert_eq!(blocking.rows[3], source[3]);
                for capacity in [1, 7, 257] {
                    let mut decoder = DecodeConfig::default()
                        .with_admission(
                            DecodePolicy::default().with_max_input_bytes(encoded.len() as u64),
                        )
                        .job()
                        .component_feed_decoder(capacity)
                        .unwrap();
                    let mut observed = Observed::default();
                    let mut at = 0;
                    assert_eq!(
                        pump(&mut decoder, &encoded, &mut at, &mut observed, false).unwrap(),
                        ComponentFeedStatus::NeedInput
                    );
                    assert_eq!(observed.rows, blocking.rows);
                    assert_eq!(observed.groups, blocking.groups);
                    assert!(observed.complete.is_none());
                    assert_eq!(decoder.usage().input_bytes, encoded.len() as u64);
                    decoder.finish_input().unwrap();
                    assert_eq!(
                        pump(&mut decoder, &encoded, &mut at, &mut observed, false).unwrap(),
                        ComponentFeedStatus::Complete
                    );
                    // Allocation addresses differ between independent decoders.
                    observed.last_pointer = 0;
                    blocking.last_pointer = 0;
                    assert_eq!(observed, blocking);
                    for _ in 0..3 {
                        assert_eq!(decoder.advance().unwrap(), ComponentFeedStatus::Complete);
                        assert!(decoder.input_request().is_none());
                    }
                }
            }
        }
    }
}

#[test]
fn native_storage_moves_to_publication_and_back_without_copying() {
    for mut storage in [
        PixelStorage::U8(vec![1, 9, 17, 25, 33]),
        PixelStorage::U16(vec![0, 257, 1023, 65535, 37]),
    ] {
        let pointer = match &storage {
            PixelStorage::U8(v) => v.as_ptr() as usize,
            PixelStorage::U16(v) => v.as_ptr() as usize,
            _ => unreachable!(),
        };
        let element = if matches!(&storage, PixelStorage::U8(_)) {
            1
        } else {
            2
        };
        let budget = Budget::new(
            DecodePolicy::default(),
            None,
            zencodec::ResourceLimits::none(),
        )
        .unwrap();
        let mut transport = Transport {
            budget,
            retain_icc: true,
            shared: Arc::new(Mutex::new(Shared {
                input: Vec::new(),
                start: 0,
                capacity: 1,
                wanted: 0,
                eof: false,
                pending: None,
                declaration: None,
                complete: None,
                ready: false,
                returned: None,
            })),
        };
        let shared = transport.shared.clone();
        let mut operation = Box::pin(transport.row(0, 0, &mut storage, 1, 3));
        assert!(
            operation
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        {
            let mut state = shared.lock().unwrap();
            let Event::Row {
                storage: published,
                start,
                width,
                ..
            } = state.pending.take().unwrap()
            else {
                panic!("expected row");
            };
            let samples = decode::storage_row(&published, start, width).unwrap();
            let row_pointer = match samples {
                AnyPlane::U8(p) => p.row(0).unwrap().as_ptr() as usize,
                AnyPlane::U16(p) => p.row(0).unwrap().as_ptr() as usize,
                _ => unreachable!(),
            };
            assert_eq!(row_pointer, pointer + element);
            assert!(
                state.ready,
                "draining the mailbox does not acknowledge the row"
            );
            state.returned = Some(published);
            state.ready = false;
        }
        assert!(matches!(
            operation
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ));
        drop(operation);
        let returned_pointer = match &storage {
            PixelStorage::U8(v) => v.as_ptr() as usize,
            PixelStorage::U16(v) => v.as_ptr() as usize,
            _ => unreachable!(),
        };
        assert_eq!(returned_pointer, pointer);
    }
}

#[test]
fn a_pending_native_row_resumes_on_another_thread() {
    let (encoded, expected) = fixture(12, ChromaSubsampling::Yuv420, None, false);
    let mut decoder = DecodeConfig::default()
        .job()
        .component_feed_decoder(7)
        .unwrap();
    let mut observed = Observed::default();
    let mut at = 0;
    assert_eq!(
        pump(&mut decoder, &encoded, &mut at, &mut observed, true).unwrap(),
        ComponentFeedStatus::Event
    );
    let result = std::thread::spawn(move || {
        let mut check = Observed::default();
        check.event(decoder.event().unwrap().unwrap()).unwrap();
        assert_eq!(check.last_pointer, observed.last_pointer);
        assert_eq!(
            pump(&mut decoder, &encoded, &mut at, &mut observed, false).unwrap(),
            ComponentFeedStatus::NeedInput
        );
        decoder.finish_input().unwrap();
        assert_eq!(
            pump(&mut decoder, &encoded, &mut at, &mut observed, false).unwrap(),
            ComponentFeedStatus::Complete
        );
        observed
    })
    .join()
    .unwrap();
    assert_eq!(result.rows, expected);
}

#[test]
fn blocking_callback_still_accepts_non_send_owners() {
    use std::{cell::Cell, rc::Rc};
    let (encoded, _) = fixture(10, ChromaSubsampling::Yuv422, None, false);
    let rows = Rc::new(Cell::new(0));
    let count = rows.clone();
    crate::components::decode_reader(&encoded[..], DecodePolicy::default(), move |event| {
        if matches!(event, ComponentEvent::Row { .. }) {
            count.set(count.get() + 1);
        }
        Ok(())
    })
    .unwrap();
    assert!(rows.get() > 0);
}

#[test]
#[allow(deprecated)] // Preserve existing common job cancellation and policy setters.
fn limits_cancellation_and_authoritative_icc_fail_before_success() {
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Clone)]
    struct Stop(Arc<AtomicBool>);
    impl zencodec::enough::Stop for Stop {
        fn check(&self) -> Result<(), zencodec::enough::StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(zencodec::enough::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_limits(zencodec::ResourceLimits::none().with_max_memory(0))
            .component_feed_decoder(1),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
    assert!(
        DecodeConfig::default()
            .job()
            .component_feed_decoder(0)
            .is_err()
    );
    assert!(
        DecodeConfig::default()
            .job()
            .component_feed_decoder(usize::MAX)
            .is_err()
    );
    let (encoded, _) = fixture(10, ChromaSubsampling::Yuv420, None, true);
    for limits in [
        zencodec::ResourceLimits::none().with_max_input_bytes(1),
        zencodec::ResourceLimits::none().with_max_total_pixels(1),
        zencodec::ResourceLimits::none().with_max_width(1),
    ] {
        let mut decoder = DecodeConfig::default()
            .job()
            .with_limits(limits)
            .component_feed_decoder(7)
            .unwrap();
        let mut observed = Observed::default();
        let mut at = 0;
        assert!(matches!(
            pump(&mut decoder, &encoded, &mut at, &mut observed, false),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        assert!(observed.header.is_none());
        assert_eq!(observed.row_counts, [0; 4]);
        assert!(matches!(
            decoder.advance(),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
    }
    let mut refused = DecodeConfig::default()
        .job()
        .with_policy(zencodec::decode::DecodePolicy::strict())
        .component_feed_decoder(7)
        .unwrap();
    let mut observed = Observed::default();
    let mut at = 0;
    assert!(matches!(
        pump(&mut refused, &encoded, &mut at, &mut observed, false),
        Err(DecodeError::Codec(Error::Unsupported(_)))
    ));
    assert!(observed.header.is_none());
    let flag = Stop(Arc::new(AtomicBool::new(false)));
    let mut decoder = DecodeConfig::default()
        .job()
        .with_stop(zencodec::StopToken::new(flag.clone()))
        .component_feed_decoder(7)
        .unwrap();
    let mut observed = Observed::default();
    let mut at = 0;
    pump(&mut decoder, &encoded, &mut at, &mut observed, true).unwrap();
    assert_eq!(observed.header.as_ref().unwrap().2, ColorAuthority::Icc);
    assert_eq!(
        observed.header.as_ref().unwrap().3.as_deref(),
        Some(b"opaque native feed profile".as_slice())
    );
    flag.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        decoder.advance(),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert!(matches!(
        decoder.push(b"ignored"),
        Err(DecodeError::Codec(Error::Cancelled))
    ));
    assert!(decoder.header().is_none());
    assert!(observed.complete.is_none());
}

#[test]
fn a_truncated_tail_keeps_rows_provisional_and_failure_sticky() {
    let (encoded, _) = fixture(
        16,
        ChromaSubsampling::Yuv444,
        Some(LossyEntropy::SplitHuf),
        false,
    );
    let shortened = &encoded[..encoded.len() - 1];
    let mut decoder = DecodeConfig::default()
        .job()
        .component_feed_decoder(7)
        .unwrap();
    let mut observed = Observed::default();
    let mut at = 0;
    assert_eq!(
        pump(&mut decoder, shortened, &mut at, &mut observed, false).unwrap(),
        ComponentFeedStatus::NeedInput
    );
    assert!(observed.row_counts.iter().all(|&n| n > 0));
    assert!(observed.complete.is_none());
    decoder.finish_input().unwrap();
    let first = decoder.advance().unwrap_err().to_string();
    assert_eq!(decoder.advance().unwrap_err().to_string(), first);
    assert_eq!(decoder.finish_input().unwrap_err().to_string(), first);
    assert!(decoder.header().is_none());
    assert!(observed.complete.is_none());
}
