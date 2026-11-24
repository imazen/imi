use super::*;
use alloc::vec;
use std::io::{self, Cursor, SeekFrom};
use zenpixels::plane::{Plane, PlaneSpec};
use zenpixels::sample::SampleEncoding;

fn layout() -> PlanarLayout {
    let encoding = PlaneEncoding::Code(SampleEncoding::new(ChannelType::U16, 10, 0).unwrap());
    PlanarLayout::new(
        vec![
            PlaneSpec::new(PlaneRole::Luma, encoding),
            PlaneSpec::new(PlaneRole::Cb, encoding),
            PlaneSpec::new(PlaneRole::Cr, encoding),
        ],
        ChromaSubsampling::Yuv444,
        ChromaSiting::Unknown,
    )
    .unwrap()
}
fn limits() -> ComponentEncodeLimits {
    ComponentEncodeLimits::new(192 << 20, 8 << 20, 1 << 20, 1 << 20, 4 << 20)
}
fn encoder(lossy: bool) -> ImiEncoderConfig {
    if lossy {
        ImiEncoderConfig::lossy(crate::LossyQuality::Ssim2(80.0))
            .with_effort(0.0)
            .tune_lossy(|t| {
                t.aq_gamma = 0.0;
                t.color = crate::LossyColor::YCoCg;
                t.stripe_block_rows = 1;
            })
    } else {
        ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(16, 8, 64 << 20))
    }
}
struct NeverIo;
impl Read for NeverIo {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("preflight touched input/scratch")
    }
}
impl Write for NeverIo {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        panic!("preflight touched output/scratch")
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("preflight flushed output/scratch")
    }
}
impl Seek for NeverIo {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        panic!("preflight sought scratch")
    }
}
impl ComponentSource for NeverIo {
    fn group(&mut self, _: u8, _: GroupRect) -> Result<AnyPlane<'_>, StreamEncodeError> {
        panic!("preflight requested source")
    }
}
struct Cancel;
impl how_far::Stop for Cancel {
    fn check(&self) -> Result<(), how_far::StopReason> {
        Err(how_far::StopReason::Cancelled)
    }
}

#[test]
fn cancellation_and_each_aggregate_denial_precede_caller_io() {
    let layout = layout();
    let encoders: Vec<_> = (0..3).map(|_| encoder(true)).collect();
    let mut cases = vec![];
    for field in 0..5 {
        let mut l = limits();
        match field {
            0 => l.max_working_bytes = 0,
            1 => l.max_output_bytes = 0,
            2 => l.max_index_bytes = 0,
            3 => l.max_checksum_bytes = 0,
            4 => l.max_numeric_scratch_bytes = 0,
            _ => unreachable!(),
        }
        cases.push(l);
    }
    for l in cases {
        let cfg =
            ComponentEncodeConfig::new(17, 9, &layout, Cicp::new(9, 16, 9, false), &encoders, l);
        let mut scratch: [NeverIo; 3] = core::array::from_fn(|_| NeverIo);
        let result = encode_groups(
            &cfg,
            &mut NeverIo,
            NeverIo,
            &mut scratch,
            NeverIo,
            NeverIo,
            &how_far::Unstoppable,
        );
        assert!(matches!(
            result,
            Err(StreamEncodeError::Codec(Error::TooLarge))
        ));
    }
    let cfg = ComponentEncodeConfig::new(
        17,
        9,
        &layout,
        Cicp::new(9, 16, 9, false),
        &encoders,
        limits(),
    );
    let mut scratch: [NeverIo; 3] = core::array::from_fn(|_| NeverIo);
    assert!(matches!(
        encode_groups(
            &cfg,
            &mut NeverIo,
            NeverIo,
            &mut scratch,
            NeverIo,
            NeverIo,
            &Cancel
        ),
        Err(StreamEncodeError::Codec(Error::Cancelled))
    ));
    for (columns, rows) in [(0, 8), (16, 0)] {
        let encoders: Vec<_> = (0..3)
            .map(|_| {
                ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(
                    columns,
                    rows,
                    64 << 20,
                ))
            })
            .collect();
        let cfg = ComponentEncodeConfig::new(
            17,
            9,
            &layout,
            Cicp::new(9, 16, 9, false),
            &encoders,
            limits(),
        );
        assert!(matches!(
            encode_groups(
                &cfg,
                &mut NeverIo,
                NeverIo,
                &mut scratch,
                NeverIo,
                NeverIo,
                &how_far::Unstoppable
            ),
            Err(StreamEncodeError::Codec(Error::InvalidInput(
                "component group geometry must be nonzero"
            )))
        ));
    }
}

#[test]
fn aggregate_allowance_never_raises_a_plane_working_limit() {
    let layout = layout();
    let encoders: Vec<_> = (0..3)
        .map(|_| ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(16, 8, 0)))
        .collect();
    let cfg = ComponentEncodeConfig::new(
        17,
        9,
        &layout,
        Cicp::new(9, 16, 9, false),
        &encoders,
        limits(),
    );
    let mut scratch: [NeverIo; 3] = core::array::from_fn(|_| NeverIo);
    assert!(matches!(
        encode_groups(
            &cfg,
            &mut NeverIo,
            NeverIo,
            &mut scratch,
            NeverIo,
            NeverIo,
            &how_far::Unstoppable
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
}

struct Source {
    kind: u8,
    calls: usize,
    bytes: Vec<u8>,
    words: Vec<u16>,
    floats: Vec<f32>,
}
impl Source {
    fn new(kind: u8) -> Self {
        Self {
            kind,
            calls: 0,
            bytes: vec![],
            words: vec![],
            floats: vec![],
        }
    }
}
impl ComponentSource for Source {
    fn group(&mut self, role: u8, rect: GroupRect) -> Result<AnyPlane<'_>, StreamEncodeError> {
        self.calls += 1;
        let width = rect.width as usize - usize::from(self.kind == 2);
        let stride = width + 3;
        let n = stride * (rect.height as usize - 1) + width;
        Ok(match self.kind {
            0 => {
                self.bytes.resize(n, 0);
                AnyPlane::U8(
                    Plane::new(&self.bytes[..], width, rect.height as usize, stride).unwrap(),
                )
            }
            1 => {
                self.floats.resize(n, 0.0);
                AnyPlane::F32(
                    Plane::new(&self.floats[..], width, rect.height as usize, stride).unwrap(),
                )
            }
            _ => {
                self.words.resize(n, 0xa5a5);
                for y in 0..rect.height as usize {
                    for x in 0..width {
                        self.words[y * stride + x] = if self.kind == 3 {
                            1024
                        } else {
                            ((rect.x as usize + x) * 37
                                + (rect.y as usize + y) * 19
                                + role as usize * 71) as u16
                                & 1023
                        };
                    }
                }
                AnyPlane::U16(
                    Plane::new(&self.words[..], width, rect.height as usize, stride).unwrap(),
                )
            }
        })
    }
}

#[test]
fn wrong_sample_types_shape_and_codes_never_publish_a_unit() {
    let layout = layout();
    let encoders: Vec<_> = (0..3).map(|_| encoder(false)).collect();
    for kind in 0..4 {
        let cfg = ComponentEncodeConfig::new(
            17,
            9,
            &layout,
            Cicp::new(9, 16, 9, false),
            &encoders,
            limits(),
        );
        let mut source = Source::new(kind);
        let mut out = vec![];
        let mut numeric: Vec<_> = (0..3).map(|_| Cursor::new(vec![])).collect();
        let result = encode_groups(
            &cfg,
            &mut source,
            &mut out,
            &mut numeric,
            Cursor::new(vec![]),
            Cursor::new(vec![]),
            &how_far::Unstoppable,
        );
        assert!(matches!(
            result,
            Err(StreamEncodeError::Codec(Error::InvalidInput(_)))
        ));
        assert_eq!(source.calls, 1);
        let mut rows = 0;
        let mut completed = false;
        let result = super::super::decode_reader(&out[..], DecodePolicy::default(), |event| {
            match event {
                super::super::ComponentEvent::Row { .. } => rows += 1,
                super::super::ComponentEvent::Complete { .. } => completed = true,
                _ => {}
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(rows, 0);
        assert!(!completed);
    }
}

struct ReadFailure(Cursor<Vec<u8>>);
impl Read for ReadFailure {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "native scratch witness",
        ))
    }
}
impl Write for ReadFailure {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl Seek for ReadFailure {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

#[test]
fn caller_scratch_prefixes_survive_and_replay_read_failure_keeps_its_class() {
    let layout = layout();
    for lossy in [false, true] {
        let encoders: Vec<_> = (0..3).map(|_| encoder(lossy)).collect();
        let cfg = ComponentEncodeConfig::new(
            17,
            9,
            &layout,
            Cicp::new(9, 16, 9, false),
            &encoders,
            limits(),
        );
        let mut numeric: Vec<_> = (0..3).map(|_| Cursor::new(vec![7; 13])).collect();
        let mut source = Source::new(4);
        let (bytes, index, checksum) = encode_groups(
            &cfg,
            &mut source,
            vec![],
            &mut numeric,
            Cursor::new(vec![8; 17]),
            Cursor::new(vec![9; 19]),
            &how_far::Unstoppable,
        )
        .unwrap();
        assert_eq!(&index.into_inner()[..17], &[8; 17]);
        assert_eq!(&checksum.into_inner()[..19], &[9; 19]);
        assert!(numeric.iter().all(|s| s.get_ref()[..13] == [7; 13]));
        let mut complete = false;
        super::super::decode_reader(&bytes[..], DecodePolicy::default(), |event| {
            if let super::super::ComponentEvent::Complete {
                encoded_integrity_verified,
                decoded_attestation,
            } = event
            {
                assert!(encoded_integrity_verified);
                assert!(!decoded_attestation);
                complete = true;
            }
            Ok(())
        })
        .unwrap();
        assert!(complete);
    }
    let encoders: Vec<_> = (0..3).map(|_| encoder(false)).collect();
    let cfg = ComponentEncodeConfig::new(
        17,
        9,
        &layout,
        Cicp::new(9, 16, 9, false),
        &encoders,
        limits(),
    );
    let mut numeric: Vec<_> = (0..3).map(|_| Cursor::new(vec![])).collect();
    let mut source = Source::new(4);
    let result = encode_groups(
        &cfg,
        &mut source,
        vec![],
        &mut numeric,
        ReadFailure(Cursor::new(vec![])),
        Cursor::new(vec![]),
        &how_far::Unstoppable,
    );
    assert!(
        matches!(result, Err(StreamEncodeError::Read(e)) if e.kind() == io::ErrorKind::PermissionDenied && e.to_string() == "native scratch witness")
    );
}
