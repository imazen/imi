//! Reference rows must reproduce the existing full-raster inter decoder.
use super::*;
use crate::read::input::Rows;

struct ReferenceRows<'a> {
    reference: &'a [u8],
    output: Vec<u8>,
    next: u32,
    reads: usize,
    fail_at: Option<u32>,
}
impl Rows for ReferenceRows<'_> {
    fn has_reference(&self) -> bool {
        true
    }
    async fn reference(&mut self, info: &ImageInfo, y: u32, out: &mut [u8]) -> Result<(), Error> {
        if self.fail_at == Some(y) {
            return Err(Error::Read);
        }
        let rb = info.row_bytes();
        assert_eq!(out.len(), rb);
        out.copy_from_slice(&self.reference[y as usize * rb..][..rb]);
        self.reads += 1;
        Ok(())
    }
    async fn row(&mut self, _: &ImageInfo, y: u32, row: &[u8]) -> Result<(), Error> {
        assert_eq!(y, self.next);
        self.next += 1;
        self.output.extend_from_slice(row);
        Ok(())
    }
}

#[test]
fn inter_reference_rows_match_full_raster() {
    check_inter_reference_rows(false);
}

pub(super) fn check_inter_reference_rows(v4: bool) {
    use crate::decode_resources::{DecodePolicy, DecodeSession};
    use crate::{Channels, ImiEncoderConfig, SampleType};
    for (lossy, entropy) in [
        (false, crate::LossyEntropy::RunLevel),
        (true, crate::LossyEntropy::RunLevel),
        (true, crate::LossyEntropy::SplitHuf),
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for sample in [SampleType::U8, SampleType::U16] {
                let info = ImageInfo::new(67, 75, channels, sample);
                let rb = info.row_bytes();
                let reference: Vec<u8> =
                    (0..rb * 75).map(|i| ((i * 71) ^ (i / 17)) as u8).collect();
                let mut current = reference.clone();
                current[rb * 34 + 5] ^= 13;
                let config = if lossy {
                    ImiEncoderConfig::lossy(90.0).tune_lossy(|t| t.entropy = entropy)
                } else {
                    ImiEncoderConfig::default().tune_lossless(|t| t.stripe_rows = Some(16))
                }
                .with_column_groups(Some(32));
                let bytes = encode_inter(
                    &config.lower(),
                    &current,
                    &info,
                    &reference,
                    &how_far::NoPulse,
                )
                .unwrap();
                let bytes = if v4 {
                    crate::v4::upgrade_image(&bytes, &Default::default()).unwrap()
                } else {
                    bytes
                };
                let mut expected = reference.clone();
                crate::v1::decode_into_inter(&bytes, &mut expected, rb, true, None, true).unwrap();
                let mut rows = ReferenceRows {
                    reference: &reference,
                    output: Vec::new(),
                    next: 0,
                    reads: 0,
                    fail_at: None,
                };
                DecodeSession::new(DecodePolicy::default())
                    .decode_reader_rows(bytes.as_slice(), &mut rows)
                    .unwrap();
                assert_eq!(
                    rows.output, expected,
                    "lossy={lossy} {channels:?} {sample:?}"
                );
                assert_eq!(rows.next, info.height);
                if !lossy || channels == Channels::Rgb {
                    assert_eq!(rows.reads, info.height as usize);
                    let mut failed = ReferenceRows {
                        reference: &reference,
                        output: Vec::new(),
                        next: 0,
                        reads: 0,
                        fail_at: Some(20),
                    };
                    assert_eq!(
                        DecodeSession::new(DecodePolicy::default())
                            .decode_reader_rows(bytes.as_slice(), &mut failed),
                        Err(Error::Read)
                    );
                }
            }
        }
    }
}

#[allow(dead_code)]
pub(super) fn suspended_v4_reference() {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    use std::sync::{Arc, Mutex};
    struct Suspended {
        reference: Vec<u8>,
        output: Arc<Mutex<Vec<u8>>>,
    }
    async fn suspend() {
        let mut ready = false;
        core::future::poll_fn(|cx| {
            if ready {
                Poll::Ready(())
            } else {
                ready = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }
    impl Rows for Suspended {
        fn has_reference(&self) -> bool {
            true
        }
        async fn reference(
            &mut self,
            info: &ImageInfo,
            y: u32,
            out: &mut [u8],
        ) -> Result<(), Error> {
            suspend().await;
            let at = y as usize * info.row_bytes();
            out.copy_from_slice(&self.reference[at..at + info.row_bytes()]);
            Ok(())
        }
        async fn row(&mut self, _: &ImageInfo, _: u32, row: &[u8]) -> Result<(), Error> {
            suspend().await;
            self.output.lock().unwrap().extend_from_slice(row);
            Ok(())
        }
    }
    for config in [
        crate::ImiEncoderConfig::lossless(),
        crate::ImiEncoderConfig::lossy(90.0),
        crate::ImiEncoderConfig::lossy(90.0)
            .tune_lossy(|t| t.entropy = crate::LossyEntropy::SplitHuf),
    ] {
        let info = ImageInfo::new(67, 75, crate::Channels::Rgb, crate::SampleType::U8);
        let reference: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 71 + i / 17) as u8)
            .collect();
        let mut current = reference.clone();
        current[info.row_bytes() * 34 + 5] ^= 13;
        let native = encode_inter(
            &config.with_column_groups(Some(32)).lower(),
            &current,
            &info,
            &reference,
            &how_far::NoPulse,
        )
        .unwrap();
        let mut expected = reference.clone();
        crate::v1::decode_into_inter(&native, &mut expected, info.row_bytes(), true, None, true)
            .unwrap();
        let file = crate::v4::upgrade_image(&native, &Default::default()).unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut future = Box::pin(crate::read::read_owned(
            std::io::Cursor::new(file),
            crate::Limits::DEFAULT,
            crate::parse::limits::Unrestricted,
            Suspended {
                reference,
                output: Arc::clone(&output),
            },
        ));
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        let rows = info.height;
        std::thread::spawn(move || {
            for _ in 0..rows * 3 + 4 {
                if let Poll::Ready(result) = future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    result.unwrap();
                    return;
                }
            }
            panic!("reference/output backpressure did not finish");
        })
        .join()
        .unwrap();
        assert!(output.lock().unwrap().as_slice() == expected.as_slice());
    }
}
