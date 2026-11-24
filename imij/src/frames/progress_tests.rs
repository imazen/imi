use super::*;
use how_far::{Outcome, StopReason};
use how_far_along::{FnPulse, Snapshot, Status};

fn phase<'a>(snapshot: &'a Snapshot, name: &str) -> Option<&'a Snapshot> {
    if snapshot.name == name {
        Some(snapshot)
    } else {
        snapshot.children.iter().find_map(|p| phase(p, name))
    }
}

#[test]
fn frame_lossless_cancels_inside_intra_and_reference_analysis_and_coding() {
    let info = ImageInfo::new(97, 41, crate::Channels::Rgb, crate::SampleType::U8);
    let pixels: Vec<u8> = (0..97 * 41 * 3).map(|i| (i * 29 + i / 131) as u8).collect();
    for reference in [false, true] {
        for target in ["pixel analysis", "unit coding"] {
            for threads in [1, 4] {
                let cfg = crate::ImiEncoderConfig::default()
                    .with_threads(threads)
                    .tune_lossless(|t| t.stripe_rows = Some(4));
                let mut encoder = FrameEncoder::animation(
                    info.clone(),
                    Metadata::default(),
                    FrameConfig::new(cfg),
                );
                if reference {
                    let prior: Vec<u8> = pixels.iter().map(|p| p ^ 19).collect();
                    encoder
                        .push_frame(&prior, 291, FrameDuration::ZERO)
                        .unwrap();
                }
                let pulse = FnPulse::new("frame", move |checkpoint| {
                    if checkpoint.phase_name() == target
                        && phase(&checkpoint.snapshot(), target).is_some_and(|p| p.completed >= 1)
                    {
                        Err(StopReason::Cancelled)
                    } else {
                        Ok(())
                    }
                });
                let observer = pulse.observer();
                assert_eq!(
                    encoder.push_frame_checked(&pixels, 291, FrameDuration::ZERO, &pulse),
                    Err(Error::Cancelled)
                );
                pulse.finish(Outcome::Cancelled).unwrap();
                let snapshot = observer.snapshot();
                let stopped = phase(&snapshot, target).unwrap();
                assert!(stopped.completed >= 1);
                assert_eq!(stopped.status, Status::Finished(Outcome::Cancelled));
                if target == "pixel analysis" {
                    assert_eq!(stopped.completed, 1);
                    assert_eq!(
                        phase(&snapshot, "unit coding").unwrap().status,
                        Status::Finished(Outcome::NotRun)
                    );
                }
                assert!(encoder.frames.is_empty());
            }
        }
    }
}
