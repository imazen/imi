//! Exercise owned, bounded transport queues rather than revealing a complete
//! backing slice. The decoder holds no reference into the producer's bytes.
use super::*;
use core::future::{Future, poll_fn};
use core::task::{Context, Poll, Waker};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Queue {
    bytes: VecDeque<u8>,
    eof: bool,
    pulled: usize,
    failure: bool,
    interrupts: usize,
}
struct Fragmented(Arc<Mutex<Queue>>);
impl Input for Fragmented {
    async fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        poll_fn(|_| {
            let mut q = self.0.lock().unwrap();
            if out.is_empty() {
                return Poll::Ready(Ok(0));
            }
            if q.interrupts != 0 {
                q.interrupts -= 1;
                return Poll::Ready(Err(std::io::ErrorKind::Interrupted.into()));
            }
            let n = q.bytes.len().min(out.len()).min(7);
            if n != 0 {
                for b in &mut out[..n] {
                    *b = q.bytes.pop_front().unwrap();
                }
                q.pulled += n;
                Poll::Ready(Ok(n))
            } else if q.failure {
                Poll::Ready(Err(std::io::Error::other("transport failed")))
            } else if q.eof {
                Poll::Ready(Ok(0))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

fn source<R>(r: R, bound: Option<u64>) -> Src<R> {
    Src {
        r,
        max: u64::MAX,
        budget: u64::MAX,
        pos: 0,
        pulled: 0,
        end: bound,
        walk: None,
        tail: Tail::None,
        ck: None,
        ck_unknown: None,
        version: crate::parse::header::HEADER_VERSION,
    }
}

fn drive<T>(
    future: impl Future<Output = T>,
    q: &Arc<Mutex<Queue>>,
    bytes: &[u8],
    step: usize,
) -> T {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    let mut at = 0;
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
        let consumed = q.lock().unwrap().pulled;
        for _ in 0..3 {
            assert!(future.as_mut().poll(&mut cx).is_pending());
            assert_eq!(q.lock().unwrap().pulled, consumed);
        }
        let mut q = q.lock().unwrap();
        assert!(!q.eof, "explicit EOF must not remain Pending");
        assert!(q.bytes.is_empty());
        if at == bytes.len() {
            q.eof = true;
        } else {
            let end = (at + step).min(bytes.len());
            q.bytes.extend(&bytes[at..end]);
            assert!(q.bytes.len() <= step);
            at = end;
        }
    }
}

#[test]
fn fragmented_records_preserve_admission_hints_and_tail() {
    use crate::parse::records::{R_STRIPE, put_record};
    let body: Vec<u8> = (0..291).map(|v| (v % 251) as u8).collect();
    let mut data = Vec::new();
    put_record(&mut data, crate::registry::side_of(R_STRIPE), b"side");
    put_record(&mut data, R_STRIPE, &body);
    for step in [1, 17, 64] {
        for cut in 0..=data.len() {
            for open in [false, true] {
                let bound = (!open).then_some(cut as u64);
                let mut expected = source(&data[..cut], bound);
                let mut expected_out = vec![99];
                let expected_count = core::cell::Cell::new(0);
                let er = expected.record_admitted(&mut expected_out, R_STRIPE, &|_| {
                    expected_count.set(expected_count.get() + 1);
                    Ok(())
                });
                let q = Arc::new(Mutex::new(Queue::default()));
                let mut actual = source(Fragmented(q.clone()), bound);
                let mut out = vec![99];
                let count = core::cell::Cell::new(0);
                let ar = drive(
                    actual.record_admitted_async(&mut out, R_STRIPE, &|_| {
                        count.set(count.get() + 1);
                        Ok(())
                    }),
                    &q,
                    &data[..cut],
                    step,
                );
                assert_eq!(ar, er, "cut {cut}, step {step}, open {open}");
                assert_eq!(out, expected_out);
                if ar.is_ok() {
                    assert_eq!(actual.pos, expected.pos);
                }
                // Failed take() may have reused different capacities; pulled
                // is the authoritative consumed-input charge on both paths.
                assert_eq!(actual.pulled, expected.pulled);
                assert_eq!(count.get(), expected_count.get());
                if cut == data.len() {
                    assert_eq!(out, body);
                }
            }
        }
    }
    let q = Arc::new(Mutex::new(Queue::default()));
    let mut src = source(Fragmented(q.clone()), Some(data.len() as u64));
    src.tail = Tail::Keep(1);
    src.begin_records(Some(vec![data.len() as u64])).unwrap();
    let mut out = Vec::new();
    drive(src.record_async(&mut out, R_STRIPE), &q, &data, 3).unwrap();
    assert_eq!(src.end_records().unwrap(), Some(vec![data.len() as u64]));
    assert_eq!(out, body);
}

fn check(table: Option<crate::checksum::OwnedTable>) -> Box<Check> {
    Box::new(Check {
        log2: 10,
        algo_id: Some(crate::HashAlgorithm::Xxh3_64.wire()),
        covered: 0,
        state: crate::Hasher::new(crate::HashAlgorithm::Xxh3_64),
        pending: Vec::new(),
        table,
        checked: 0,
        ahead: Vec::new(),
        ahead_pos: 0,
    })
}

#[test]
fn fragmented_checksum_read_ahead_and_late_table() {
    let bytes: Vec<u8> = (0..2109).map(|i| (i % 251) as u8).collect();
    let mut digests = Vec::new();
    for chunk in bytes.chunks(1024) {
        // Independent checksum implementation, not Check::cover's state.
        digests.extend_from_slice(&xxhash_rust::xxh3::xxh3_64(chunk).to_le_bytes());
    }
    let table = crate::checksum::Table {
        algo: crate::HashAlgorithm::Xxh3_64,
        width: 8,
        log2: 10,
        covered_len: bytes.len() as u64,
        digests: &digests,
    };
    for early in [false, true] {
        for corrupt in [false, true] {
            let mut input = bytes.clone();
            if corrupt {
                input[1030] ^= 1;
            }
            let q = Arc::new(Mutex::new(Queue::default()));
            let mut src = source(Fragmented(q.clone()), None);
            src.ck = Some(check(
                early.then(|| crate::checksum::OwnedTable::new(&table).unwrap()),
            ));
            let mut out = vec![0; bytes.len()];
            let result = drive(src.exact_async(&mut out), &q, &input, 31);
            if early && corrupt {
                assert_eq!(result, Err(crate::checksum::MISMATCH));
                continue;
            }
            result.unwrap();
            assert_eq!(out, input);
            let ck = src.ck.as_mut().unwrap();
            if !early {
                let result = ck.set_table(table, &mut src.r);
                if corrupt {
                    assert_eq!(result, Err(crate::checksum::MISMATCH));
                    continue;
                }
                result.unwrap();
            }
            ck.finish().unwrap();
            assert_eq!(ck.checked, 3);
        }
    }
}

#[test]
fn fragmented_tags_exclude_checksum_and_keep_clean_eof() {
    let bytes = b"DATAcSUMTEXT";
    let q = Arc::new(Mutex::new(Queue::default()));
    let mut src = source(Fragmented(q.clone()), None);
    src.ck = Some(check(None));
    let tags = drive(
        async {
            let mut tags = Vec::new();
            while let Some(tag) = src.tag_async().await? {
                tags.push(tag);
            }
            Ok::<_, Error>(tags)
        },
        &q,
        bytes,
        1,
    )
    .unwrap();
    assert_eq!(tags, [*b"DATA", *b"cSUM", *b"TEXT"]);
    let ck = src.ck.unwrap();
    assert_eq!(ck.covered, 8);
    assert_eq!(
        ck.state.finish().low_u64(),
        xxhash_rust::xxh3::xxh3_64(b"DATATEXT")
    );
    for cut in 1..4 {
        let q = Arc::new(Mutex::new(Queue::default()));
        let mut src = source(Fragmented(q.clone()), None);
        assert_eq!(
            drive(src.tag_async(), &q, &bytes[..cut], 1),
            Err(Error::Corrupt("truncated file"))
        );
    }
}

#[test]
fn fragmented_take_bounds_growth_and_accounts_errors() {
    let q = Arc::new(Mutex::new(Queue::default()));
    let mut src = source(Fragmented(q.clone()), Some(1));
    let mut out = Vec::new();
    assert_eq!(
        drive(src.take_async(1 << 30, &mut out), &q, &[], 1),
        Err(Error::Corrupt("read past DATA boundary"))
    );
    assert_eq!(out.capacity(), 0);
    assert_eq!(q.lock().unwrap().pulled, 0);

    let q = Arc::new(Mutex::new(Queue::default()));
    let mut src = source(Fragmented(q.clone()), None);
    let mut out = Vec::new();
    let future = src.take_async(1 << 30, &mut out);
    fn require_send<T: Send>(_: &T) {}
    require_send(&future);
    assert_eq!(
        drive(future, &q, b"abc", 1),
        Err(Error::Corrupt("truncated file"))
    );
    assert!(out.is_empty());
    assert!(
        out.capacity() <= 16384,
        "declared length must not reserve the record"
    );
    assert_eq!(src.pulled, 3);
    assert_eq!(src.pos, 3);

    let q = Arc::new(Mutex::new(Queue {
        bytes: VecDeque::from([1, 2, 3]),
        failure: true,
        ..Queue::default()
    }));
    let mut src = source(Fragmented(q.clone()), None);
    let mut out = Vec::new();
    assert_eq!(
        payload::ready(src.take_async(10, &mut out)),
        Err(Error::Read)
    );
    assert_eq!(src.pulled, 3);
    assert!(out.is_empty());
}

pub(super) fn head(
    header: &crate::parse::header::Header,
    declared: crate::parse::header::Features,
    data: &[u8],
    step: usize,
    open: bool,
) -> Result<(Head, usize), Error> {
    let q = Arc::new(Mutex::new(Queue::default()));
    let mut src = source(Fragmented(q.clone()), (!open).then_some(data.len() as u64));
    let result = drive(
        async {
            payload::read_head_async(
                header,
                declared,
                &mut StreamHead::new(&mut src),
                &crate::parse::limits::Unrestricted,
            )
            .await
        },
        &q,
        data,
        step,
    );
    result.map(|head| (head, src.pos as usize))
}

#[test]
fn fragmented_budget_boundary_and_interrupted_reads() {
    for input in [b"x".as_slice(), b"xy".as_slice()] {
        let q = Arc::new(Mutex::new(Queue {
            interrupts: 2,
            ..Queue::default()
        }));
        let mut src = source(Fragmented(q.clone()), None);
        src.budget = 1;
        let result = drive(
            async {
                let mut byte = [0];
                src.exact_async(&mut byte).await?;
                assert_eq!(byte, *b"x");
                src.try_u8_async().await
            },
            &q,
            input,
            1,
        );
        assert_eq!(
            result,
            if input.len() == 1 {
                Ok(None)
            } else {
                Err(Error::TooLarge)
            }
        );
        assert_eq!(src.pulled, 1);
        // Existing budget-boundary semantics probe one extra byte only to
        // distinguish clean EOF from input beyond the allowed file length.
        assert_eq!(q.lock().unwrap().pulled, input.len());
    }
    let q = Arc::new(Mutex::new(Queue::default()));
    let mut src = source(Fragmented(q.clone()), None);
    src.budget = 3;
    let mut out = Vec::new();
    assert_eq!(
        drive(src.take_async(4, &mut out), &q, b"abcd", 1),
        Err(Error::TooLarge)
    );
    assert_eq!(q.lock().unwrap().pulled, 0);
    assert_eq!(out.capacity(), 0);

    let q = Arc::new(Mutex::new(Queue {
        interrupts: 2,
        ..Queue::default()
    }));
    let mut src = source(Fragmented(q.clone()), None);
    let mut out = Vec::new();
    drive(src.take_async(3, &mut out), &q, b"abc", 1).unwrap();
    assert_eq!(out, b"abc");
    assert_eq!(src.pulled, 3);
    assert_eq!(src.pos, 3);
}

#[derive(Default)]
struct OutputState {
    waiting: bool,
    permit: bool,
    fail_at: Option<usize>,
    rows: Vec<(ImageInfo, u32, Vec<u8>)>,
}
struct Backpressure(Arc<Mutex<OutputState>>);
impl input::Rows for Backpressure {
    async fn row(&mut self, info: &ImageInfo, y: u32, pixels: &[u8]) -> Result<(), Error> {
        self.0.lock().unwrap().waiting = true;
        poll_fn(|_| {
            let mut state = self.0.lock().unwrap();
            if !state.permit {
                return Poll::Pending;
            }
            state.permit = false;
            state.waiting = false;
            if state.fail_at == Some(state.rows.len()) {
                return Poll::Ready(Err(Error::Cancelled));
            }
            state.rows.push((info.clone(), y, pixels.to_vec()));
            Poll::Ready(Ok(()))
        })
        .await
    }
}

/// Full framing and reconstruction with independent input/output suspension.
/// This remains a test transport, not the production caller-feed executor.
pub(super) fn file_pixels(file: &[u8]) {
    let expected = crate::decode(file).unwrap();
    let expected_meta = crate::probe(file).unwrap().1;
    for step in [1, 7, 64] {
        let (result, output) = caller_feed(file, step);
        assert_eq!(result, Ok((expected.0.clone(), expected_meta.clone())));
        assert_eq!(output, expected.1);
    }
    for step in [1, 7, 64] {
        let (result, rows, consumed) = file_result(file, step);
        assert_eq!(result, Ok((expected.0.clone(), expected_meta.clone())));
        assert_eq!(consumed, file.len() as u64);
        assert_eq!(rows.len(), expected.0.height as usize);
        let mut pixels = Vec::new();
        for (y, (info, ordinal, row)) in rows.into_iter().enumerate() {
            assert_eq!(info, expected.0);
            assert_eq!(ordinal, y as u32);
            pixels.extend_from_slice(&row);
        }
        assert_eq!(pixels, expected.1);
    }
}

fn caller_feed(file: &[u8], capacity: usize) -> (Result<(ImageInfo, Metadata), Error>, Vec<u8>) {
    use crate::codec::{DecodeConfig, DecodeError, FeedStatus};
    use zencodec::decode::DecoderConfig;
    let mut decoder = DecodeConfig::default()
        .job()
        .feed_decoder(capacity)
        .unwrap();
    let mut at = 0;
    let mut output = Vec::new();
    let mut eof = false;
    loop {
        match decoder.advance() {
            Ok(FeedStatus::NeedInput) => {
                assert!(!eof, "explicit EOF must be terminal");
                assert_eq!(decoder.advance().unwrap(), FeedStatus::NeedInput);
                if at == file.len() {
                    decoder.finish_input().unwrap();
                    eof = true;
                } else {
                    at += decoder.push(&file[at..]).unwrap();
                }
            }
            Ok(FeedStatus::Row { .. }) => {
                output.extend_from_slice(decoder.row().unwrap().unwrap().row(0))
            }
            Ok(FeedStatus::Complete) => {
                let m = decoder.native_metadata().unwrap();
                return (Ok((m.image.clone(), m.metadata.clone())), output);
            }
            Err(DecodeError::Codec(e)) => return (Err(e), output),
            Err(DecodeError::Sink(_)) => panic!("no external sink"),
        }
    }
}

type DeliveredRows = Vec<(ImageInfo, u32, Vec<u8>)>;
fn file_result(
    file: &[u8],
    step: usize,
) -> (Result<(ImageInfo, Metadata), Error>, DeliveredRows, u64) {
    file_result_with_failure(file, step, None)
}

fn file_result_with_failure(
    file: &[u8],
    step: usize,
    fail_at: Option<usize>,
) -> (Result<(ImageInfo, Metadata), Error>, DeliveredRows, u64) {
    let q = Arc::new(Mutex::new(Queue::default()));
    let output = Arc::new(Mutex::new(OutputState {
        fail_at,
        ..Default::default()
    }));
    let mut src = source(Fragmented(q.clone()), None);
    let mut sink = Backpressure(output.clone());
    let admission = crate::parse::limits::Unrestricted;
    let result = {
        let mut future = core::pin::pin!(read_file(&mut src, &Limits::NONE, &admission, &mut sink));
        let mut cx = Context::from_waker(Waker::noop());
        let mut at = 0;
        loop {
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                break result;
            }
            let consumed = q.lock().unwrap().pulled;
            let delivered = output.lock().unwrap().rows.len();
            for _ in 0..3 {
                assert!(future.as_mut().poll(&mut cx).is_pending());
                assert_eq!(q.lock().unwrap().pulled, consumed);
                assert_eq!(output.lock().unwrap().rows.len(), delivered);
            }
            let mut state = output.lock().unwrap();
            if state.waiting {
                state.permit = true;
            } else {
                let mut input = q.lock().unwrap();
                assert!(!input.eof, "explicit EOF must complete");
                assert!(input.bytes.is_empty());
                if at == file.len() {
                    input.eof = true;
                } else {
                    let end = (at + step).min(file.len());
                    input.bytes.extend(&file[at..end]);
                    assert!(input.bytes.len() <= step);
                    at = end;
                }
            }
        }
    };
    let rows = core::mem::take(&mut output.lock().unwrap().rows);
    (result, rows, src.pulled)
}

#[test]
fn fragmented_file_checksums_metadata_and_every_eof() {
    let info = ImageInfo::new(9, 7, crate::Channels::Rgb, crate::SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * 7)
        .map(|i| (i * 37 % 251) as u8)
        .collect();
    let file = crate::encode(
        &pixels,
        info.row_bytes(),
        &info,
        &Metadata::default(),
        &crate::EncodeConfig::default(),
    )
    .unwrap();
    let mut file = crate::add_pixel_checksum(file).unwrap();
    // Legal late metadata must arrive with final success, after provisional rows.
    file.extend_from_slice(b"iCCP");
    crate::put_varint(&mut file, 5);
    file.extend_from_slice(b"test");
    file_pixels(&file);
    let checksummed = crate::encode(
        &pixels,
        info.row_bytes(),
        &info,
        &Metadata {
            icc: Some(b"test".to_vec()),
            ..Default::default()
        },
        &crate::EncodeConfig {
            checksum: true,
            ..Default::default()
        },
    )
    .unwrap();
    let checksummed = crate::add_pixel_checksum(checksummed).unwrap();
    file_pixels(&checksummed);
    for input in [&file, &checksummed] {
        for fail_at in [0, 3, 6] {
            let mut expected_rows = Vec::new();
            let (expected, pulled) = decode_reader_with(
                &input[..],
                &Limits::NONE,
                u64::MAX,
                &crate::parse::limits::Unrestricted,
                &mut |info, y, row| {
                    if expected_rows.len() == fail_at {
                        return Err(Error::Cancelled);
                    }
                    expected_rows.push((info.clone(), y, row.to_vec()));
                    Ok(())
                },
            );
            let (actual, rows, consumed) = file_result_with_failure(input, 3, Some(fail_at));
            assert_eq!(actual, Err(Error::Cancelled));
            assert_eq!(actual, expected);
            assert_eq!(rows, expected_rows);
            assert_eq!(consumed, pulled);
        }
    }
    // Corrupted payload must retain checksum-error precedence under suspension.
    let mut bad = checksummed.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    let (actual, _, _) = file_result(&bad, 3);
    let expected = decode_reader(&bad[..], &Limits::NONE, |_, _, _| Ok(()));
    assert!(expected.is_err());
    assert_eq!(actual, expected);
    assert_eq!(caller_feed(&bad, 3).0, expected);
    for cut in 0..=file.len() {
        let mut expected_rows = Vec::new();
        let (expected, pulled) = decode_reader_with(
            &file[..cut],
            &Limits::NONE,
            u64::MAX,
            &crate::parse::limits::Unrestricted,
            &mut |i, y, row| {
                expected_rows.push((i.clone(), y, row.to_vec()));
                Ok(())
            },
        );
        let (actual, rows, consumed) = file_result(&file[..cut], 7);
        assert_eq!(actual, expected, "cut {cut}");
        assert_eq!(rows, expected_rows, "cut {cut}");
        assert_eq!(consumed, pulled, "cut {cut}");
    }
}
