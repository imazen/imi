//! Public API controls, independent of the private driver's output wrapper.
use imij::stream_encode::{
    BoundedLossySession, BoundedLossyStep, BoundedStream, StreamEncodeError,
};
use imij::{Channels, Error, ImageInfo, ImiEncoderConfig, LossyQuality, Metadata, SampleType};

fn configuration() -> ImiEncoderConfig {
    ImiEncoderConfig::lossy(LossyQuality::Ssim2(80.0))
        .with_effort(0.0)
        .with_checksum(false)
        .tune_lossy(|t| {
            t.aq_gamma = 0.0;
            t.stripe_block_rows = 1;
        })
}

#[test]
fn frame_cap_refuses_a_large_header_before_exposing_output() {
    let info = ImageInfo::new(17, 17, Channels::Rgb, SampleType::U8);
    // Valid XML with nonrepeating hex content remains larger than the file
    // cap even if the metadata writer compresses it.
    let mut xml = String::from("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><data>");
    let mut state = 0x12345678u32;
    for _ in 0..8192 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        use core::fmt::Write;
        write!(&mut xml, "{:02x}", state as u8).unwrap();
    }
    xml.push_str("</data></x:xmpmeta>");
    let metadata = Metadata {
        xmp: Some(xml.into_bytes()),
        ..Metadata::default()
    };
    let mut session = BoundedLossySession::one_pass_budgeted(
        info,
        &metadata,
        &configuration(),
        1 << 20,
        32 << 20,
        1024,
        1 << 20,
    )
    .unwrap();
    let mut source_rows = 0;
    loop {
        match session.step() {
            Ok(BoundedLossyStep::Rows { pass, y0, rows }) => {
                assert_eq!((pass, y0), (2, 0));
                rows.fill(0);
                source_rows += rows.len() / (17 * 3);
            }
            Ok(BoundedLossyStep::Write {
                stream: BoundedStream::Output,
                bytes,
                ..
            }) => {
                panic!("oversized header exposed {} bytes", bytes.len());
            }
            Err(StreamEncodeError::Codec(Error::TooLarge)) => break,
            _ => panic!("unexpected request or completion before oversized-header refusal"),
        }
    }
    assert_eq!(source_rows, 8);
}

#[test]
fn caller_supplied_aq_remains_an_explicit_refusal() {
    let info = ImageInfo::new(17, 17, Channels::Rgb, SampleType::U8);
    let cfg = configuration().tune_lossy(|t| t.lab.aq_map = Some(vec![64u8; 9].into()));
    assert!(matches!(
        BoundedLossySession::one_pass(info.clone(), &Metadata::default(), &cfg, 1 << 20, 32 << 20),
        Err(StreamEncodeError::Codec(Error::Unsupported(
            "bounded lossy encoding excludes AQ"
        )))
    ));
    assert!(matches!(
        BoundedLossySession::one_pass_budgeted(
            info,
            &Metadata::default(),
            &cfg,
            1 << 20,
            32 << 20,
            1 << 20,
            1 << 20
        ),
        Err(StreamEncodeError::Codec(Error::Unsupported(
            "bounded lossy encoding excludes AQ"
        )))
    ));
}

#[test]
fn oversized_geometry_refuses_before_any_session_request() {
    let info = ImageInfo::new(u32::MAX, u32::MAX, Channels::Rgba, SampleType::U16);
    assert!(matches!(
        BoundedLossySession::one_pass_budgeted(
            info,
            &Metadata::default(),
            &configuration(),
            1 << 20,
            32 << 20,
            1 << 20,
            1 << 20
        ),
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
}
