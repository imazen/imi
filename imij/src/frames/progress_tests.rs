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

#[test]
fn frame_encoder_reuses_prev_src_and_skips_buffer_when_inter_disabled() {
    let info = ImageInfo::new(41, 29, crate::Channels::Rgba, crate::SampleType::U8);
    let rb = info.row_bytes();
    let stride = rb + 12;
    let mut f0 = vec![0xCCu8; stride * 29];
    for y in 0..29 {
        for x in 0..rb {
            f0[y * stride + x] = (x * 13 + y * 7) as u8;
        }
    }
    let mut f1 = f0.clone();
    f1[5 * stride + 12] ^= 0xFF;

    let mut no_inter_cfg = FrameConfig::new(crate::ImiEncoderConfig::lossy(80.0).with_threads(1));
    no_inter_cfg.inter = false;
    let mut no_inter = FrameEncoder::animation(info.clone(), Metadata::default(), no_inter_cfg);
    no_inter
        .push_frame(&f0, stride, FrameDuration::from_millis(33))
        .unwrap();
    no_inter
        .push_frame(&f1, stride, FrameDuration::from_millis(33))
        .unwrap();
    assert!(no_inter.prev_src.is_empty());
    assert_eq!(no_inter.prev_src.capacity(), 0);

    let mut inter_enc = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(crate::ImiEncoderConfig::lossless().with_threads(1)),
    );
    inter_enc
        .push_frame(&f0, stride, FrameDuration::from_millis(33))
        .unwrap();
    let ptr = inter_enc.prev_src.as_ptr();
    let cap = inter_enc.prev_src.capacity();
    assert_eq!(inter_enc.prev_src.len(), rb * 29);
    // Cancelling frame 1 must leave prev_src untouched and not push a frame.
    let before_cancel = inter_enc.prev_src.clone();
    let cancel = FnPulse::new("cancel", |_| Err(StopReason::Cancelled));
    assert_eq!(
        inter_enc.push_frame_checked(&f1, stride, FrameDuration::from_millis(33), &cancel),
        Err(Error::Cancelled)
    );
    assert!(inter_enc.frames.is_empty());
    assert_eq!(inter_enc.prev_src, before_cancel);

    inter_enc
        .push_frame(&f1, stride, FrameDuration::from_millis(33))
        .unwrap();
    assert_eq!(inter_enc.prev_src.as_ptr(), ptr);
    assert_eq!(inter_enc.prev_src.capacity(), cap);
    assert_eq!(inter_enc.prev_src[5 * rb + 12], f1[5 * stride + 12]);
}

#[test]
fn changed_rect_handles_edges_full_width_fast_path_and_strides() {
    let stop = &how_far::NoPulse;
    for bpp in [1usize, 3, 4, 8] {
        let (w, h) = (17usize, 13usize);
        let rb = w * bpp;
        let a_stride = rb + 5;
        let b_stride = rb + 11;
        let mut a = vec![0xAAu8; a_stride * h];
        let mut b = vec![0x55u8; b_stride * h];
        for y in 0..h {
            for x in 0..rb {
                let v = ((x * 29 + y * 17) & 0xFF) as u8;
                a[y * a_stride + x] = v;
                b[y * b_stride + x] = v;
            }
        }
        // Identical active pixels with differing stride padding -> None.
        assert_eq!(
            changed_rect(&a, a_stride, &b, b_stride, w, h, bpp, stop).unwrap(),
            None
        );

        // Single pixel at each corner and interior (only changing last byte of pixel).
        for (px, py) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1), (7, 5)] {
            let mut c = b.clone();
            c[py * b_stride + px * bpp + (bpp - 1)] ^= 1;
            assert_eq!(
                changed_rect(&a, a_stride, &c, b_stride, w, h, bpp, stop).unwrap(),
                Some((px, py, px + 1, py + 1)),
                "bpp={bpp} corner=({px},{py})"
            );
        }

        // Row 2 hits full width (x0=0, x1=w) -> triggers bottom-up y1 scan.
        let mut full_w = b.clone();
        full_w[2 * b_stride] ^= 1;
        full_w[2 * b_stride + rb - 1] ^= 1;
        assert_eq!(
            changed_rect(&a, a_stride, &full_w, b_stride, w, h, bpp, stop).unwrap(),
            Some((0, 2, w, 3))
        );
        full_w[9 * b_stride + 5 * bpp] ^= 1;
        assert_eq!(
            changed_rect(&a, a_stride, &full_w, b_stride, w, h, bpp, stop).unwrap(),
            Some((0, 2, w, 10))
        );

        // 1x1 canvas differing and equal.
        let p0 = vec![7u8; bpp + 3];
        let mut p1 = vec![9u8; bpp + 7];
        p1[..bpp].copy_from_slice(&p0[..bpp]);
        assert_eq!(
            changed_rect(&p0, bpp + 3, &p1, bpp + 7, 1, 1, bpp, stop).unwrap(),
            None
        );
        p1[bpp - 1] ^= 1;
        assert_eq!(
            changed_rect(&p0, bpp + 3, &p1, bpp + 7, 1, 1, bpp, stop).unwrap(),
            Some((0, 0, 1, 1))
        );
    }
}
