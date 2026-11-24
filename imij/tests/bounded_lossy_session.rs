//! `stream_encode::BoundedLossySession`: the sans-IO form of
//! `encode_lossy_bounded`, the same bytes for the same arguments, every
//! source band asked for top to bottom per pass, and caller failures.

use imij::stream_encode::{
    BoundedLossyConfig, BoundedLossyPlan, BoundedLossySession, BoundedLossyStep, BoundedStream,
    LossyUnitIndex, StreamEncodeError, encode_lossy_bounded,
};
use imij::{Channels, DecodeSpeed, Error, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use std::io::Cursor;

/// Gradients and texture, flat white panels (exact blocks qualify there)
/// and text-like strokes.
fn raster(info: &ImageInfo) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let bps = info.sample.bytes();
    let mut out = vec![0u8; w * h * n * bps];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let v = match (x / 23 + y / 17) % 4 {
                    0 => 255,
                    1 if (x + y) % 5 < 2 => 20,
                    1 => 240,
                    _ => (((x * 5 + y * 3 + c * 70) ^ (x * y / 7)) % 256) as u8,
                };
                let at = ((y * w + x) * n + c) * bps;
                if bps == 2 {
                    out[at..at + 2].copy_from_slice(&(v as u16 * 257).to_ne_bytes());
                } else {
                    out[at] = v;
                }
            }
        }
    }
    out
}

fn blocking(
    info: &ImageInfo,
    src: &[u8],
    lossy: &ImiEncoderConfig,
    index: LossyUnitIndex,
) -> Vec<u8> {
    let budget = 64 << 20;
    let (out, _, _) = encode_lossy_bounded(
        &mut Cursor::new(src),
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        lossy.checksum().then(|| Cursor::new(Vec::new())),
        info.clone(),
        &Metadata::default(),
        lossy,
        BoundedLossyConfig::new(info.row_bytes() as u64, index, budget),
        &how_far::NoPulse,
    )
    .unwrap();
    out.into_inner()
}

fn put(v: &mut Vec<u8>, at: u64, bytes: &[u8]) {
    let end = at as usize + bytes.len();
    if v.len() < end {
        v.resize(end, 0);
    }
    v[at as usize..end].copy_from_slice(bytes);
}

/// Drives a session over three vectors; returns the output and every
/// `Rows` step as (pass, y0, rows).
fn session(
    info: &ImageInfo,
    src: &[u8],
    lossy: &ImiEncoderConfig,
    index: LossyUnitIndex,
) -> (Vec<u8>, Vec<(u8, u32, usize)>) {
    let s = BoundedLossySession::new(info.clone(), &Metadata::default(), lossy, index, 64 << 20)
        .unwrap();
    drive(s, info, src)
}

fn drive(
    mut s: BoundedLossySession,
    info: &ImageInfo,
    src: &[u8],
) -> (Vec<u8>, Vec<(u8, u32, usize)>) {
    let rb = info.row_bytes();
    let (mut out, mut idx, mut sums) = (Vec::new(), Vec::new(), Vec::new());
    let mut rows_log = Vec::new();
    loop {
        match s.step().unwrap() {
            BoundedLossyStep::Rows { pass, y0, rows } => {
                rows.copy_from_slice(&src[y0 as usize * rb..][..rows.len()]);
                rows_log.push((pass, y0, rows.len() / rb));
            }
            BoundedLossyStep::Write {
                stream,
                offset,
                bytes,
            } => match stream {
                BoundedStream::Output => put(&mut out, offset, bytes),
                BoundedStream::IndexScratch => put(&mut idx, offset, bytes),
                BoundedStream::ChecksumScratch => put(&mut sums, offset, bytes),
                _ => unreachable!(),
            },
            BoundedLossyStep::Read {
                stream,
                offset,
                buf,
            } => {
                let v = match stream {
                    BoundedStream::Output => &out,
                    BoundedStream::IndexScratch => &idx,
                    BoundedStream::ChecksumScratch => &sums,
                    _ => unreachable!(),
                };
                buf.copy_from_slice(&v[offset as usize..][..buf.len()]);
            }
            BoundedLossyStep::Done { len } => {
                assert_eq!(len as usize, out.len());
                return (out, rows_log);
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn sessions_write_the_blocking_encoders_bytes() {
    let rgb = ImageInfo::new(700, 420, Channels::Rgb, SampleType::U8);
    let wide = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U16);
    let rgba = ImageInfo::new(300, 200, Channels::Rgba, SampleType::U8);
    let fast = imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast);
    let compact = imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Compact);
    let mut exact = fast.clone();
    exact = exact.tune_lossy(|t| {
        t.exact_blocks = Some(1.5);
    });
    let cases = [
        ("fast", &rgb, fast.clone(), LossyUnitIndex::Reserved(4)),
        (
            "fast, trailing index",
            &rgb,
            fast.clone(),
            LossyUnitIndex::Trailing,
        ),
        (
            "compact",
            &rgb,
            compact.clone(),
            LossyUnitIndex::Reserved(4),
        ),
        (
            "exact blocks",
            &rgb,
            exact.clone(),
            LossyUnitIndex::Reserved(4),
        ),
        (
            "no tier",
            &rgb,
            imij::ImiEncoderConfig::lossy(70.0),
            LossyUnitIndex::Reserved(4),
        ),
        (
            "checksum",
            &rgb,
            fast.clone().with_checksum(true),
            LossyUnitIndex::Reserved(4),
        ),
        (
            "checksum, trailing index",
            &rgb,
            exact.clone().with_checksum(true),
            LossyUnitIndex::Trailing,
        ),
        ("u16", &wide, fast.clone(), LossyUnitIndex::Reserved(4)),
        ("rgba", &rgba, fast.clone(), LossyUnitIndex::Reserved(4)),
        // One-byte reservation: spans overflow it, so the trailing index decides.
        (
            "narrow reservation",
            &rgb,
            fast.clone(),
            LossyUnitIndex::Reserved(1),
        ),
    ];
    for (name, info, lossy, index) in cases {
        let src = raster(info);
        let expected = blocking(info, &src, &lossy, index);
        let (got, rows) = session(info, &src, &lossy, index);
        assert!(
            got == expected,
            "{name}: {} vs {} bytes",
            got.len(),
            expected.len()
        );
        // Every pass covers the image once, top to bottom.
        let mut passes: Vec<u8> = rows.iter().map(|r| r.0).collect();
        passes.dedup();
        for pass in passes {
            let mut y = 0;
            for &(p, y0, n) in &rows {
                if p == pass {
                    assert_eq!(y0, y, "{name}: pass {pass}");
                    y += n as u32;
                }
            }
            assert_eq!(y, info.height, "{name}: pass {pass}");
        }
        assert_eq!(
            imij::decode(&got).unwrap().1,
            imij::decode(&expected).unwrap().1
        );
    }
}

#[test]
fn a_failed_step_stops_the_session_with_the_callers_error() {
    let info = ImageInfo::new(700, 420, Channels::Rgb, SampleType::U8);
    let src = raster(&info);
    let lossy = imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast);
    // Fail the first source band, then the first output write.
    for fail_on_write in [false, true] {
        let mut s = BoundedLossySession::new(
            info.clone(),
            &Metadata::default(),
            &lossy,
            LossyUnitIndex::Reserved(4),
            64 << 20,
        )
        .unwrap();
        let rb = info.row_bytes();
        loop {
            match s.step() {
                Ok(BoundedLossyStep::Rows { y0, rows, .. }) => {
                    if !fail_on_write {
                        s.fail(StreamEncodeError::Read(std::io::Error::other("source")));
                        continue;
                    }
                    rows.copy_from_slice(&src[y0 as usize * rb..][..rows.len()]);
                }
                Ok(BoundedLossyStep::Write { .. }) if fail_on_write => {
                    s.fail(StreamEncodeError::Write(std::io::Error::other("sink")));
                }
                Ok(BoundedLossyStep::Done { .. }) => panic!("finished despite a failure"),
                Ok(_) => {}
                Err(StreamEncodeError::Read(e)) => {
                    assert!(!fail_on_write);
                    assert_eq!(e.to_string(), "source");
                    break;
                }
                Err(StreamEncodeError::Write(e)) => {
                    assert!(fail_on_write);
                    assert_eq!(e.to_string(), "sink");
                    break;
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    }
}

#[test]
fn sessions_refuse_what_the_encoder_refuses_before_any_step() {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let mut small = imij::ImiEncoderConfig::lossy(70.0);
    small = small.tune_lossy(|t| {
        t.small_transforms = true;
    });
    let r = BoundedLossySession::new(
        info,
        &Metadata::default(),
        &small,
        LossyUnitIndex::Reserved(4),
        1 << 20,
    );
    assert!(matches!(
        r.err(),
        Some(StreamEncodeError::Codec(Error::Unsupported(_)))
    ));
}

#[test]
fn prepared_session_moves_after_a_filled_request_without_copying_it() {
    fn assert_send<T: Send>() {}
    assert_send::<BoundedLossySession>();
    assert_send::<BoundedLossyPlan>();
    let info = ImageInfo::new(129, 97, Channels::Rgb, SampleType::U8);
    let src = raster(&info);
    let cfg = ImiEncoderConfig::lossy(70.0)
        .with_decode_speed(DecodeSpeed::Fast)
        .with_checksum(true);
    let (expected, expected_rows) = session(&info, &src, &cfg, LossyUnitIndex::Trailing);
    let plan = BoundedLossyPlan::new(
        info.clone(),
        Metadata::default(),
        &cfg,
        LossyUnitIndex::Trailing,
        64 << 20,
    )
    .unwrap();
    let mut s = plan.start().unwrap();
    let first = match s.step().unwrap() {
        BoundedLossyStep::Rows { pass, y0, rows } => {
            let n = rows.len() / info.row_bytes();
            rows.copy_from_slice(&src[y0 as usize * info.row_bytes()..][..rows.len()]);
            (pass, y0, n)
        }
        _ => panic!("the first native request must be source rows"),
    };
    // The answered buffer is still owned by the suspended session. Actually
    // resume native execution on the other thread, with borrowed source data.
    let (bytes, mut rows) = std::thread::scope(|scope| {
        let info = &info;
        let src = &src;
        scope.spawn(move || drive(s, info, src)).join().unwrap()
    });
    rows.insert(0, first);
    assert_eq!(bytes, expected);
    assert_eq!(rows, expected_rows);
}

#[test]
fn first_failure_is_typed_and_terminal_even_when_not_observed_immediately() {
    #[derive(Debug)]
    struct Original;
    impl std::fmt::Display for Original {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("original source failure")
        }
    }
    impl std::error::Error for Original {}
    let info = ImageInfo::new(129, 97, Channels::Rgb, SampleType::U8);
    let cfg = ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast);
    let mut s = BoundedLossySession::new(
        info,
        &Metadata::default(),
        &cfg,
        LossyUnitIndex::Trailing,
        64 << 20,
    )
    .unwrap();
    match s.step().unwrap() {
        BoundedLossyStep::Rows { rows, .. } => rows[..1].fill(7),
        _ => panic!("expected source request"),
    }
    s.fail(StreamEncodeError::Read(std::io::Error::other(Original)));
    s.fail(StreamEncodeError::Codec(Error::Cancelled));
    let error = match s.step() {
        Err(error) => error,
        Ok(_) => panic!("must refuse partial request"),
    };
    let StreamEncodeError::Read(error) = error else {
        panic!("lost original error")
    };
    assert!(error.get_ref().unwrap().is::<Original>());
    for _ in 0..3 {
        assert!(matches!(
            s.step(),
            Err(StreamEncodeError::Codec(Error::Corrupt(_)))
        ));
    }
    let mut cancelled = BoundedLossySession::new(
        ImageInfo::new(129, 97, Channels::Rgb, SampleType::U8),
        &Metadata::default(),
        &cfg,
        LossyUnitIndex::Trailing,
        64 << 20,
    )
    .unwrap();
    cancelled.fail(StreamEncodeError::Codec(Error::Cancelled));
    assert!(matches!(
        cancelled.step(),
        Err(StreamEncodeError::Codec(Error::Cancelled))
    ));
    assert!(cancelled.step().is_err());
}

#[cfg(feature = "lab")]
#[test]
fn single_pass_sessions_move_after_answered_source_without_replaying_it() {
    let info = ImageInfo::new(65, 97, Channels::Rgba, SampleType::U16);
    let src = raster(&info);
    let cfg = ImiEncoderConfig::lossy(70.0)
        .with_effort(0.0)
        .with_checksum(true)
        .tune_lossy(|t| t.stripe_block_rows = 1);
    for budgeted in [false, true] {
        let make = || {
            if budgeted {
                BoundedLossySession::one_pass_budgeted(
                    info.clone(),
                    &Metadata::default(),
                    &cfg,
                    64 << 20,
                    64 << 20,
                    64 << 20,
                    64 << 20,
                )
            } else {
                BoundedLossySession::one_pass(
                    info.clone(),
                    &Metadata::default(),
                    &cfg,
                    64 << 20,
                    64 << 20,
                )
            }
            .unwrap()
        };
        let expected = drive(make(), &info, &src);
        let mut session = make();
        let first = match session.step().unwrap() {
            BoundedLossyStep::Rows { pass, y0, rows } => {
                assert_eq!((pass, y0), (2, 0));
                let count = rows.len() / info.row_bytes();
                rows.copy_from_slice(&src[..rows.len()]);
                (pass, y0, count)
            }
            _ => panic!("expected the first source band"),
        };
        let (bytes, mut rows) = std::thread::scope(|scope| {
            let info = &info;
            let src = &src;
            scope
                .spawn(move || drive(session, info, src))
                .join()
                .unwrap()
        });
        rows.insert(0, first);
        assert_eq!(bytes, expected.0);
        assert_eq!(rows, expected.1);
        assert!(rows.iter().all(|(pass, _, _)| *pass == 2));
    }
}
