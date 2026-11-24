//! Real allocator accounting, restricted to this test's thread. This measures
//! native preparation; it is not a whole-operation memory or stack-size bound.
#![cfg(all(feature = "std", feature = "encode"))]
#![forbid(unsafe_code)]

use allocation_counter::measure;
use imij::stream_encode::{BoundedLossyPlan, LossyUnitIndex};
use imij::{Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, Metadata, SampleType};

#[test]
fn preparation_defers_native_allocations_and_moves_existing_metadata() {
    // Verify that the allocator sees an allocation on this thread before
    // interpreting a zero result. Keep the object alive beyond measurement.
    let mut control = None;
    let observed = measure(|| control = Some(vec![7u8; 4096]));
    assert!(observed.count_total > 0 && observed.bytes_total > 0);
    std::hint::black_box(&control);
    drop(control);

    for sample in [SampleType::U8, SampleType::U16] {
        for custom_bits in [None, Some(5)] {
            for (width, height) in [(129, 97), (8192, 8192)] {
                let mut info = ImageInfo::new(width, height, Channels::Rgb, sample);
                if let Some(bits) = custom_bits {
                    info = info.with_significant_bits(&[bits; 3]).unwrap();
                }
                let cfg = ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast);
                // Pre-existing caller metadata is moved, with its allocation
                // retained. Its construction is deliberately outside the scope.
                let metadata = Metadata {
                    exif: Some(vec![1, 2, 3, 4]),
                    ..Metadata::default()
                };
                let ptr = metadata.exif.as_ref().unwrap().as_ptr();
                let mut plan = None;
                let stats = measure(|| {
                    plan = Some(
                        BoundedLossyPlan::new(
                            info,
                            metadata,
                            &cfg,
                            LossyUnitIndex::Trailing,
                            64 << 20,
                        )
                        .unwrap(),
                    );
                });
                assert_eq!(
                    stats.count_total, 0,
                    "{sample:?} {custom_bits:?} {width}x{height}: {stats:?}"
                );
                assert_eq!(stats.bytes_total, 0);
                let plan = plan.unwrap();
                assert_eq!(plan.metadata().exif.as_ref().unwrap().as_ptr(), ptr);
                let passes = plan.passes();
                let mut session = None;
                let started = measure(|| session = Some(plan.start().unwrap()));
                // Positive control on the actual native path: start allocates
                // the future/mailbox, and conversion tables for these U16 cases.
                assert!(started.count_total > 0 && started.bytes_total > 0);
                assert_eq!(session.as_ref().unwrap().passes(), passes);
                println!(
                    "{sample:?} {custom_bits:?} {width}x{height}: preparation={stats:?}, start={started:?}"
                );
                drop(session);
            }
        }
    }
}
