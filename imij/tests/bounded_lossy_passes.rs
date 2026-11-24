//! `BoundedLossySession::passes`: the planned source passes and request size,
//! read before the first step, bound what every encode actually requests.
//! Content decides between `min` and `max`, and both are reached. The coding
//! pass is the last, and no output byte is written before every earlier pass
//! has finished.

use imij::stream_encode::{
    BoundedLossyPasses, BoundedLossySession, BoundedLossyStep, BoundedStream, LossyUnitIndex,
};
use imij::{Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, Metadata, SampleType};

/// Gradients and texture, flat white panels (exact blocks qualify there)
/// and text-like strokes.
fn graphic(info: &ImageInfo) -> Vec<u8> {
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

/// A smooth horizontal 8-bit gradient: no block qualifies as exact.
fn gradient(info: &ImageInfo) -> Vec<u8> {
    let (w, n) = (info.width as usize, info.channels.count());
    (0..w * info.height as usize * n)
        .map(|i| ((i / n) % w * 200 / w) as u8)
        .collect()
}

fn put(v: &mut Vec<u8>, at: u64, bytes: &[u8]) {
    let end = at as usize + bytes.len();
    if v.len() < end {
        v.resize(end, 0);
    }
    v[at as usize..end].copy_from_slice(bytes);
}

/// What one encode requested, by step ordinal.
struct Run {
    /// Read from the session before its first step.
    planned: BoundedLossyPasses,
    /// Pass labels in request order, one per source pass (each starts at row 0).
    passes: Vec<u8>,
    /// Largest `Rows` request.
    max_rows: u32,
    /// Ordinal of the last `Rows` step of a pass other than coding.
    last_analysis_rows: Option<usize>,
    /// Ordinal of the first coding-pass `Rows` step.
    first_coding_rows: Option<usize>,
    /// Ordinal of the first output write.
    first_output: Option<usize>,
    source_rows: Vec<(u8, u32, u32)>,
    output: Vec<u8>,
}

fn run(info: &ImageInfo, src: &[u8], cfg: &ImiEncoderConfig) -> Run {
    let s = BoundedLossySession::new(
        info.clone(),
        &Metadata::default(),
        cfg,
        LossyUnitIndex::Reserved(4),
        64 << 20,
    )
    .unwrap();
    drive(info, src, s)
}

fn drive(info: &ImageInfo, src: &[u8], mut s: BoundedLossySession) -> Run {
    let rb = info.row_bytes();
    let planned = s.passes();
    let (mut out, mut idx, mut sums) = (Vec::new(), Vec::new(), Vec::new());
    let mut r = Run {
        planned,
        passes: Vec::new(),
        max_rows: 0,
        last_analysis_rows: None,
        first_coding_rows: None,
        first_output: None,
        source_rows: Vec::new(),
        output: Vec::new(),
    };
    for n in 0.. {
        match s.step().unwrap() {
            BoundedLossyStep::Rows { pass, y0, rows } => {
                rows.copy_from_slice(&src[y0 as usize * rb..][..rows.len()]);
                if y0 == 0 {
                    r.passes.push(pass);
                }
                let count = (rows.len() / rb) as u32;
                let expected = r
                    .source_rows
                    .last()
                    .filter(|(p, _, _)| *p == pass)
                    .map_or(0, |(_, y, n)| y + n);
                assert_eq!(
                    y0, expected,
                    "source rows must be sequential within each pass"
                );
                r.source_rows.push((pass, y0, count));
                r.max_rows = r.max_rows.max(count);
                if pass == 2 {
                    r.first_coding_rows.get_or_insert(n);
                } else {
                    r.last_analysis_rows = Some(n);
                }
            }
            BoundedLossyStep::Write {
                stream,
                offset,
                bytes,
            } => match stream {
                BoundedStream::Output => {
                    r.first_output.get_or_insert(n);
                    put(&mut out, offset, bytes);
                }
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
                break;
            }
            _ => unreachable!(),
        }
    }
    r.output = out;
    assert_eq!(s.passes(), planned, "planning is stable after completion");
    for pass in &r.passes {
        assert_eq!(
            r.source_rows
                .iter()
                .filter(|(p, _, _)| p == pass)
                .map(|(_, _, n)| n)
                .sum::<u32>(),
            info.height
        );
    }
    r
}

/// Checks the guarantees `passes()` documents; returns the observed count.
fn check(name: &str, r: &Run, height: u32) -> u32 {
    let seen = r.passes.len() as u32;
    let p = r.planned;
    assert!(
        p.min <= seen && seen <= p.max,
        "{name}: {seen} passes {:?} outside planned {p:?}",
        r.passes
    );
    let mut labels = r.passes.clone();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(
        labels.len(),
        r.passes.len(),
        "{name}: a pass label repeated"
    );
    assert_eq!(r.passes.last(), Some(&2), "{name}: coding is the last pass");
    assert_eq!(p.band_rows, r.max_rows, "{name}: band rows");
    assert!(p.band_rows <= height, "{name}");
    let first_output = r.first_output.expect("output");
    if let Some(last) = r.last_analysis_rows {
        assert!(last < first_output, "{name}: output before analysis ended");
        assert!(last < r.first_coding_rows.unwrap(), "{name}");
    }
    seen
}

#[test]
fn planned_passes_bound_every_encode_and_content_reaches_both_ends() {
    let rgb = ImageInfo::new(420, 330, Channels::Rgb, SampleType::U8);
    let rgba = ImageInfo::new(300, 200, Channels::Rgba, SampleType::U8);
    let wide = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U16);
    // More than 64 Ki pixels: bounded trellis efforts refuse smaller images.
    let trellis = ImageInfo::new(320, 240, Channels::Rgb, SampleType::U16);
    let fast = ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast);
    let exact = fast.clone().tune_lossy(|t| t.exact_blocks = Some(1.5));
    let compact = ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Compact);

    // No exact blocks (off on these tiers, or not 8-bit) and no lab rate
    // model switch: no content-dependent pass, exactly as planned.
    let fixed: [(&str, &ImageInfo, ImiEncoderConfig); 5] = [
        ("fast", &rgb, fast.clone()),
        (
            "default rgba checksum",
            &rgba,
            ImiEncoderConfig::lossy(40.0).with_checksum(true),
        ),
        ("fast rgb16", &wide, fast.clone()),
        (
            "trellis effort 9",
            &trellis,
            ImiEncoderConfig::lossy(70.0).with_effort(9.0),
        ),
        (
            "trellis effort 10",
            &trellis,
            ImiEncoderConfig::lossy(70.0).with_effort(10.0),
        ),
    ];
    for (name, info, cfg) in &fixed {
        let r = run(info, &graphic(info), cfg);
        assert_eq!(r.planned.min, r.planned.max, "{name}: {:?}", r.planned);
        assert_eq!(check(name, &r, info.height), r.planned.min, "{name}");
        if name.starts_with("trellis") {
            assert!(r.passes.iter().any(|&p| p >= 10), "{name}: {:?}", r.passes);
        }
    }

    // Compact selects exact blocks by default; whether its nested pass is
    // content-dependent follows from its DC stripe geometry. Every content
    // stays within the plan.
    for content in [graphic(&rgb), gradient(&rgb)] {
        let r = run(&rgb, &content, &compact);
        check("compact", &r, rgb.height);
    }

    // Exact blocks on a tier whose DC stripes fit a work band: the nested
    // pass runs only when blocks are selected. Same configuration, same
    // geometry: one content reaches `max`, the other `min`.
    let selected = run(&rgb, &graphic(&rgb), &exact);
    let none = run(&rgb, &gradient(&rgb), &exact);
    assert_eq!(
        selected.planned, none.planned,
        "planned from geometry and configuration"
    );
    assert_eq!(selected.planned.max, selected.planned.min + 1);
    assert_eq!(
        check("exact, graphic", &selected, rgb.height),
        selected.planned.max
    );
    assert!(selected.passes.contains(&3), "{:?}", selected.passes);
    assert_eq!(
        check("exact, gradient", &none, rgb.height),
        none.planned.min
    );
    assert!(!none.passes.contains(&3), "{:?}", none.passes);
}

#[cfg(feature = "lab")]
#[test]
fn single_source_traversal_is_planned_for_plain_and_budgeted_streaming() {
    use imij::LossyEntropy;
    for sample in [SampleType::U8, SampleType::U16] {
        for channels in [Channels::Gray, Channels::Rgb, Channels::Rgba] {
            for stripe in [1, 8, 16] {
                for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    for columns in [None, Some(32)] {
                        let info = ImageInfo::new(65, 129, channels, sample);
                        let cfg = ImiEncoderConfig::lossy(70.0)
                            .with_effort(0.0)
                            .with_checksum(true)
                            .with_column_groups(columns)
                            .tune_lossy(|t| {
                                t.stripe_block_rows = stripe;
                                t.entropy = entropy;
                            });
                        let src = graphic(&info);
                        for budgeted in [false, true] {
                            let session = if budgeted {
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
                            .unwrap();
                            let r = drive(&info, &src, session);
                            assert_eq!((r.planned.min, r.planned.max), (1, 1));
                            assert_eq!(r.planned.band_rows, (stripe * 8).min(info.height));
                            assert_eq!(r.passes, [2], "native pass label is not pass count");
                            assert!(r.last_analysis_rows.is_none());
                            assert_eq!(check("single-pass", &r, info.height), 1);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn retained_native_plan_keeps_declaration_metadata_and_lowered_configuration() {
    use imij::stream_encode::BoundedLossyPlan;
    fn is_send<T: Send>() {}
    is_send::<BoundedLossyPlan>();
    for sample in [SampleType::U8, SampleType::U16] {
        let info = ImageInfo::new(65, 33, Channels::Rgba, sample);
        let mut cfg = ImiEncoderConfig::lossy(70.0)
            .with_effort(0.0)
            .with_checksum(true);
        let original_cfg = cfg.clone();
        let src = graphic(&info);
        let metadata = Metadata {
            xmp: Some(b"<xmp>retained plan</xmp>".to_vec()),
            ..Metadata::default()
        };
        let original_metadata = metadata.clone();
        let metadata_pointer = metadata.xmp.as_ref().unwrap().as_ptr();
        let plan = BoundedLossyPlan::new(
            info.clone(),
            metadata,
            &cfg,
            LossyUnitIndex::Trailing,
            64 << 20,
        )
        .unwrap();
        assert_eq!(plan.info(), &info);
        assert_eq!(plan.metadata(), &original_metadata);
        assert_eq!(
            plan.metadata().xmp.as_ref().unwrap().as_ptr(),
            metadata_pointer,
            "native preparation moves metadata without cloning its payload"
        );
        let passes = plan.passes();
        cfg = ImiEncoderConfig::lossy(10.0).with_effort(0.0);
        drop(cfg); // The caller may replace and then discard its configuration.
        let expected = drive(
            &info,
            &src,
            BoundedLossySession::new(
                info.clone(),
                &original_metadata,
                &original_cfg,
                LossyUnitIndex::Trailing,
                64 << 20,
            )
            .unwrap(),
        );
        let actual = std::thread::spawn(move || {
            let session = plan.start().unwrap();
            assert_eq!(session.passes(), passes);
            drive(&info, &src, session)
        })
        .join()
        .unwrap();
        assert_eq!(
            actual.output, expected.output,
            "starting a retained plan uses its original configuration and metadata"
        );
    }
}

#[test]
fn retained_preparation_rejects_bad_index_admission_without_starting() {
    use imij::stream_encode::BoundedLossyPlan;
    let info = ImageInfo::new(65, 33, Channels::Rgb, SampleType::U8);
    let cfg = ImiEncoderConfig::lossy(70.0).with_effort(0.0);
    for index in [LossyUnitIndex::Reserved(0), LossyUnitIndex::Reserved(9)] {
        assert!(
            BoundedLossyPlan::new(info.clone(), Metadata::default(), &cfg, index, 64 << 20)
                .is_err()
        );
    }
    assert!(
        BoundedLossyPlan::new(info, Metadata::default(), &cfg, LossyUnitIndex::Trailing, 0)
            .is_err()
    );
}
