//! Decode progress through a `Pulse`: one child per lossless unit, exact
//! pixels, and cancellation between units.

use how_far::{Outcome, StopReason, Unstoppable, prelude::*};
use how_far_along::{FnPulse, Phase, PulseTree, Snapshot, Status, Total};
use imij::codec::DecodeConfig;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};
use zencodec::decode::{Decode, DecodeJob, DecoderConfig};

fn image(column_groups: Option<u32>) -> (ImageInfo, Vec<u8>, Vec<u8>) {
    let info = ImageInfo::new(97, 41, Channels::Rgb, SampleType::U8);
    let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 29 + i / 131) as u8)
        .collect();
    let file = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(0.0);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(8);
        });
        cfg = cfg.with_column_groups(column_groups);
        cfg
    }
    .encode_samples(
        imij::ImageRef::new(&raw, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap();
    (info, raw, file)
}

fn units(snapshot: &Snapshot) -> Vec<&Snapshot> {
    snapshot
        .children
        .iter()
        .filter(|c| c.name.starts_with("stripe "))
        .collect()
}

fn check_units(snapshot: &Snapshot, expected: usize, height: u64) {
    let units = units(snapshot);
    assert_eq!(
        units.len(),
        expected,
        "{:?}",
        snapshot
            .children
            .iter()
            .map(|c| &c.name)
            .collect::<Vec<_>>()
    );
    for u in &units {
        assert_eq!(u.status, Status::Finished(Outcome::Succeeded), "{}", u.name);
        assert_eq!(u.units, "rows");
    }
    let groups = expected as u64 / height.div_ceil(8);
    assert_eq!(
        units.iter().map(|u| u.completed).sum::<u64>(),
        height * groups
    );
}

#[test]
fn zencodec_decode_reports_every_unit_with_exact_pixels() {
    for (column_groups, expected) in [(None, 6), (Some(32), 24)] {
        let (info, raw, file) = image(column_groups);
        let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
        let observer = tree.observer();
        let out = DecodeConfig::default()
            .job()
            .with_pulse(tree.share().unwrap())
            .decoder(std::borrow::Cow::Borrowed(&file[..]), &[])
            .unwrap()
            .decode();
        let out = out.unwrap();
        tree.finish(Outcome::Succeeded).unwrap();
        for y in 0..info.height {
            let row = info.row_bytes();
            assert_eq!(
                out.pixels().row(y),
                &raw[y as usize * row..(y as usize + 1) * row]
            );
        }
        check_units(&observer.snapshot(), expected, u64::from(info.height));
    }
}

#[test]
fn zencodec_decode_cancels_between_units() {
    let (_, _, file) = image(None);
    let pulse = FnPulse::new("decode", |checkpoint| {
        let snapshot = checkpoint.snapshot();
        if units(&snapshot)
            .iter()
            .any(|u| u.status == Status::Finished(Outcome::Succeeded))
        {
            Err(StopReason::Cancelled)
        } else {
            Ok(())
        }
    });
    let observer = pulse.observer();
    let result = DecodeConfig::default()
        .job()
        .with_pulse(pulse.share().unwrap())
        .decoder(std::borrow::Cow::Borrowed(&file[..]), &[])
        .unwrap()
        .decode();
    assert!(result.is_err());
    pulse.finish(Outcome::Cancelled).unwrap();
    let snapshot = observer.snapshot();
    let units = units(&snapshot);
    assert_eq!(units.len(), 6);
    assert_eq!(units[0].status, Status::Finished(Outcome::Succeeded));
    assert!(
        units
            .iter()
            .skip(2)
            .all(|u| u.status == Status::Finished(Outcome::NotRun))
    );
}

#[cfg(feature = "lab")]
#[test]
fn threaded_decode_reports_every_unit_with_exact_pixels() {
    for (column_groups, expected) in [(None, 6), (Some(32), 24)] {
        let (info, raw, file) = image(column_groups);
        for threads in [1, 4] {
            let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
            let observer = tree.observer();
            let mut out = vec![0u8; raw.len()];
            let decoded = imij::lab::decode_into_threads_with_pulse(
                &file,
                &mut out,
                info.row_bytes(),
                threads,
                &tree,
            );
            let decoded = decoded.finish_phase(tree).unwrap();
            assert_eq!(decoded, info);
            assert_eq!(out, raw);
            check_units(&observer.snapshot(), expected, u64::from(info.height));
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn threaded_quick_decode_reports_every_stripe() {
    let (info, raw, _) = image(None);
    let mut cfg = imij::ImiEncoderConfig::quick();
    cfg = cfg.tune_lossless(|t| t.stripe_rows = Some(8));
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    for threads in [1, 4] {
        let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
        let observer = tree.observer();
        let mut out = vec![0u8; raw.len()];
        let decoded = imij::lab::decode_into_threads_with_pulse(
            &file,
            &mut out,
            info.row_bytes(),
            threads,
            &tree,
        );
        decoded.finish_phase(tree).unwrap();
        assert_eq!(out, raw);
        check_units(&observer.snapshot(), 6, u64::from(info.height));
    }
}

#[test]
fn unobserved_decode_matches() {
    let (info, raw, file) = image(Some(32));
    let mut out = vec![0u8; raw.len()];
    imij::decode_into_threads(&file, &mut out, info.row_bytes(), 4).unwrap();
    assert_eq!(out, raw);
    let _ = Error::Cancelled;
}

fn lossy_image(column_groups: Option<u32>) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(97, 41, Channels::Rgb, SampleType::U8);
    let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 29 + i / 131) as u8)
        .collect();
    let mut cfg = imij::ImiEncoderConfig::lossy(80.0);
    cfg = cfg.tune_lossy(|t| {
        t.stripe_block_rows = 1;
    });
    cfg = cfg.with_column_groups(column_groups);
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    (info, file)
}

#[test]
fn zencodec_lossy_decode_reports_every_unit_with_the_same_pixels() {
    for (column_groups, expected) in [(None, 6), (Some(32), 24)] {
        let (info, file) = lossy_image(column_groups);
        let plain = DecodeConfig::default()
            .job()
            .decoder(std::borrow::Cow::Borrowed(&file[..]), &[])
            .unwrap()
            .decode()
            .unwrap();
        let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
        let observer = tree.observer();
        let out = DecodeConfig::default()
            .job()
            .with_pulse(tree.share().unwrap())
            .decoder(std::borrow::Cow::Borrowed(&file[..]), &[])
            .unwrap()
            .decode()
            .unwrap();
        tree.finish(Outcome::Succeeded).unwrap();
        for y in 0..info.height {
            assert_eq!(out.pixels().row(y), plain.pixels().row(y));
        }
        check_units(&observer.snapshot(), expected, u64::from(info.height));
    }
}

#[cfg(feature = "lab")]
#[test]
fn threaded_lossy_decode_reports_every_unit_with_the_same_pixels() {
    for (column_groups, expected) in [(None, 6), (Some(32), 24)] {
        let (info, file) = lossy_image(column_groups);
        let mut plain = vec![0u8; info.checked_buffer_len().unwrap()];
        imij::decode_into_threads(&file, &mut plain, info.row_bytes(), 4).unwrap();
        for threads in [1, 4] {
            let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
            let observer = tree.observer();
            let mut out = vec![0u8; plain.len()];
            imij::lab::decode_into_threads_with_pulse(
                &file,
                &mut out,
                info.row_bytes(),
                threads,
                &tree,
            )
            .finish_phase(tree)
            .unwrap();
            assert_eq!(out, plain);
            check_units(&observer.snapshot(), expected, u64::from(info.height));
        }
    }
}

/// Native JPEG payloads (SPEC 2.12): one child per band, named as the
/// codestream trace names the band records, with the plain decode's pixels.
#[test]
#[cfg(feature = "native-jpeg")]
fn native_jpeg_decode_reports_every_band() {
    // Gray, 16x36: 5 MCU rows, one per band, the last one 4 rows high.
    let blocks = 2 * 5;
    let image = imij::jpeg::CoefficientImage {
        width: 16,
        height: 36,
        color: imij::jpeg::ComponentColor::Gray,
        siting: imij::jpeg::ChromaSiting::Unspecified,
        tables: [Some([3; 64]), None, None, None],
        components: vec![imij::jpeg::QuantizedComponent {
            info: imij::jpeg::ComponentInfo {
                id: 1,
                horizontal_sampling: 1,
                vertical_sampling: 1,
                quant_table: 0,
                blocks_wide: 2,
                blocks_high: 5,
            },
            coefficients: (0..blocks * 64)
                .map(|i| {
                    if i % 64 == 0 {
                        (i / 64) as i16 * 9 - 40
                    } else {
                        (i % 7) as i16 - 3
                    }
                })
                .collect(),
        }],
    };
    let file = image
        .encode(&Metadata::default(), 1, imij::LossyEntropy::RunLevel, 7.5)
        .unwrap();
    let (info, plain) = imij::decode(&file).unwrap();
    let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
    let observer = tree.observer();
    let out = DecodeConfig::default()
        .job()
        .with_pulse(tree.share().unwrap())
        .decoder(std::borrow::Cow::Borrowed(&file[..]), &[])
        .unwrap()
        .decode()
        .unwrap();
    tree.finish(Outcome::Succeeded).unwrap();
    for y in 0..info.height {
        let row = info.row_bytes();
        assert_eq!(
            out.pixels().row(y),
            &plain[y as usize * row..(y as usize + 1) * row]
        );
    }
    let snapshot = observer.snapshot();
    let bands: Vec<_> = snapshot
        .children
        .iter()
        .filter(|c| c.name.starts_with("band "))
        .collect();
    assert_eq!(
        bands.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        [
            "band 0 (rows 0-7)",
            "band 1 (rows 8-15)",
            "band 2 (rows 16-23)",
            "band 3 (rows 24-31)",
            "band 4 (rows 32-35)"
        ]
    );
    for b in &bands {
        assert_eq!(b.status, Status::Finished(Outcome::Succeeded), "{}", b.name);
    }
    assert_eq!(bands.iter().map(|b| b.completed).sum::<u64>(), 36);
}

/// Animation decoders advance the job's pulse by one per decoded frame.
#[test]
fn animation_decode_advances_one_per_frame() {
    use zencodec::decode::AnimationFrameDecoder;
    let info = ImageInfo::new(23, 11, Channels::Rgb, SampleType::U8);
    let mut cfg = imij::FrameConfig::new(imij::ImiEncoderConfig::default());
    cfg.inter = true;
    let mut e = imij::FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
    for t in 0..4u8 {
        let frame: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i as u8).wrapping_mul(7).wrapping_add(t * 13))
            .collect();
        e.push_frame(
            &frame,
            info.row_bytes(),
            imij::FrameDuration::from_millis(40),
        )
        .unwrap();
    }
    let file = e.finish().unwrap();
    let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
    let observer = tree.observer();
    let mut d = DecodeConfig::default()
        .job()
        .with_pulse(tree.share().unwrap())
        .animation_frame_decoder(std::borrow::Cow::Borrowed(&file[..]), &[])
        .unwrap();
    let mut frames = 0;
    while d.render_next_frame(None).unwrap().is_some() {
        frames += 1;
        assert_eq!(observer.snapshot().completed, frames);
    }
    assert_eq!(frames, 4);
    tree.finish(Outcome::Succeeded).unwrap();
}

/// Push decoders advance the job's pulse by one per row the sink receives.
#[test]
fn push_decode_advances_one_per_row() {
    use zencodec::decode::DecodeRowSink;
    use zenpixels::{PixelDescriptor, PixelSliceMut};
    struct Rows {
        bytes: Vec<u8>,
        stride: usize,
        width: u32,
        observer: how_far_along::Observer,
        seen: Vec<u64>,
    }
    impl DecodeRowSink for Rows {
        fn begin(
            &mut self,
            width: u32,
            height: u32,
            descriptor: PixelDescriptor,
        ) -> Result<(), zencodec::decode::SinkError> {
            self.stride = width as usize * descriptor.bytes_per_pixel();
            self.bytes = vec![0; self.stride * height as usize];
            self.width = width;
            Ok(())
        }
        fn provide_next_buffer(
            &mut self,
            y: u32,
            _height: u32,
            width: u32,
            descriptor: PixelDescriptor,
        ) -> Result<PixelSliceMut<'_>, zencodec::decode::SinkError> {
            // Rows before this one are already reported.
            self.seen.push(self.observer.snapshot().completed);
            let at = y as usize * self.stride;
            Ok(PixelSliceMut::new(
                &mut self.bytes[at..at + self.stride],
                width,
                1,
                self.stride,
                descriptor,
            )
            .unwrap())
        }
        fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
            Ok(())
        }
    }
    let (info, raw, file) = image(Some(32));
    let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
    let mut sink = Rows {
        bytes: vec![],
        stride: 0,
        width: 0,
        observer: tree.observer(),
        seen: vec![],
    };
    DecodeConfig::default()
        .job()
        .with_pulse(tree.share().unwrap())
        .push_decoder(std::borrow::Cow::Borrowed(&file[..]), &mut sink, &[])
        .unwrap();
    assert_eq!(sink.bytes, raw);
    assert_eq!(sink.seen, (0..u64::from(info.height)).collect::<Vec<_>>());
    assert_eq!(tree.observer().snapshot().completed, u64::from(info.height));
    tree.finish(Outcome::Succeeded).unwrap();
}

/// Pull row decoders advance the job's pulse by one per yielded row.
#[test]
fn row_decode_advances_one_per_row() {
    use zencodec::decode::StreamingDecode;
    let (info, raw, file) = image(None);
    let tree = PulseTree::new(Phase::new("decode", Total::Unknown), Unstoppable);
    let observer = tree.observer();
    let mut rows = DecodeConfig::default()
        .job()
        .with_pulse(tree.share().unwrap())
        .row_decoder(&file, &[])
        .unwrap();
    let mut got = Vec::new();
    while let Some((y, slice)) = rows.next_batch().unwrap() {
        assert_eq!(observer.snapshot().completed, u64::from(y) + 1);
        got.extend_from_slice(slice.row(0));
    }
    assert_eq!(got, raw);
    assert_eq!(observer.snapshot().completed, u64::from(info.height));
    drop(rows);
    tree.finish(Outcome::Succeeded).unwrap();
}
