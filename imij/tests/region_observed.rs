//! `RegionReader::decode_region_observed`: one progress child per unit of
//! the region, the same pixels as `decode_region_threads`, and cancellation
//! between units.

use how_far::{Outcome, StopReason, Unstoppable};
use how_far_along::{FnPulse, Phase, PulseTree, Snapshot, Status, Total};
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

fn raster(info: &ImageInfo) -> Vec<u8> {
    (0..info.checked_buffer_len().unwrap())
        .map(|i| ((i * 29 + i / 131) % 251) as u8)
        .collect()
}

/// A lossless and a lossy file with column groups, several stripes each.
fn files() -> Vec<(&'static str, ImageInfo, Vec<u8>)> {
    let info = ImageInfo::new(300, 200, Channels::Rgb, SampleType::U8);
    let raw = raster(&info);
    let lossless = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(0.0);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg = cfg.with_column_groups(Some(64));
        cfg
    }
    .encode_samples(
        imij::ImageRef::new(&raw, info.row_bytes(), &info),
        &Metadata::default(),
    )
    .unwrap();
    let mut lossy = imij::ImiEncoderConfig::lossy(70.0);
    lossy = lossy.with_column_groups(Some(64));
    let lossy = lossy
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    vec![("lossless", info.clone(), lossless), ("lossy", info, lossy)]
}

fn units(snapshot: &Snapshot) -> Vec<&Snapshot> {
    snapshot
        .children
        .iter()
        .filter(|c| c.name.starts_with("stripe "))
        .collect()
}

#[test]
fn observed_regions_report_each_unit_with_the_same_pixels() {
    let rect = Rect::new(70, 40, 150, 90);
    for (name, info, file) in files() {
        let bpp = info.row_bytes() / info.width as usize;
        let stride = rect.width() as usize * bpp;
        let mut expected = vec![0u8; stride * rect.height() as usize];
        RegionReader::open(SliceReader::new(&file))
            .unwrap()
            .decode_region_threads(rect, &mut expected, stride, 1)
            .unwrap();
        for threads in [1, 4] {
            let tree = PulseTree::new(Phase::new("region", Total::Unknown), Unstoppable);
            let observer = tree.observer();
            let mut out = vec![0u8; expected.len()];
            RegionReader::open(SliceReader::new(&file))
                .unwrap()
                .decode_region_observed(rect, &mut out, stride, threads, &tree)
                .unwrap();
            tree.finish(Outcome::Succeeded).unwrap();
            assert_eq!(out, expected, "{name}, {threads} threads");
            let snapshot = observer.snapshot();
            let units = units(&snapshot);
            assert!(!units.is_empty(), "{name}");
            for u in &units {
                assert_eq!(
                    u.status,
                    Status::Finished(Outcome::Succeeded),
                    "{name} {}",
                    u.name
                );
            }
        }
    }
}

#[test]
fn observed_regions_cancel_between_units() {
    let rect = Rect::new(0, 0, 300, 200);
    for (name, info, file) in files() {
        let stride = info.row_bytes();
        // Stop once the first unit has finished.
        let pulse = FnPulse::new("region", |checkpoint| {
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
        let mut out = vec![0u8; stride * info.height as usize];
        let r = RegionReader::open(SliceReader::new(&file))
            .unwrap()
            .decode_region_observed(rect, &mut out, stride, 1, &pulse);
        assert!(matches!(r, Err(Error::Cancelled)), "{name}: {r:?}");
        pulse.finish(Outcome::Cancelled).unwrap();
        let snapshot = observer.snapshot();
        let units = units(&snapshot);
        let done = units
            .iter()
            .filter(|u| u.status == Status::Finished(Outcome::Succeeded))
            .count();
        assert!(
            done >= 1 && done < units.len(),
            "{name}: {done} of {}",
            units.len()
        );
        // A stop before any read decodes nothing.
        let stop = FnPulse::new("region", |_| Err(StopReason::Cancelled));
        let r = RegionReader::open(SliceReader::new(&file))
            .unwrap()
            .decode_region_observed(rect, &mut out, stride, 1, &stop);
        assert!(matches!(r, Err(Error::Cancelled)), "{name}");
        stop.finish(Outcome::Cancelled).unwrap();
    }
}
