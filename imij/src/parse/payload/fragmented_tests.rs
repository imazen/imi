//! Fragmentation oracle for the shared grammar. The caller owns visibility and
//! EOF; polling without new bytes must not restart parser/admission work.
use super::*;
use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::task::{Context, Poll, Waker};

struct Feed<'a> {
    data: &'a [u8],
    available: &'a Cell<usize>,
    eof: &'a Cell<bool>,
    pos: usize,
}

impl AsyncHeadInput for Feed<'_> {
    async fn peek_u8(&mut self) -> Result<u8, Error> {
        poll_fn(|_| {
            if self.pos < self.available.get() {
                Poll::Ready(Ok(self.data[self.pos]))
            } else if self.eof.get() {
                Poll::Ready(Err(Error::Corrupt("truncated")))
            } else {
                Poll::Pending
            }
        })
        .await
    }

    async fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(Error::Corrupt("length overflow"))?;
        poll_fn(|_| {
            if end <= self.available.get() {
                Poll::Ready(Ok(()))
            } else if self.eof.get() {
                Poll::Ready(Err(Error::Corrupt("truncated")))
            } else {
                Poll::Pending
            }
        })
        .await?;
        let start = self.pos;
        self.pos = end;
        Ok(&self.data[start..end])
    }

    fn mark(&self) -> crate::trace::Mark {
        crate::trace::Mark::Abs(self.pos as u64)
    }
    fn pos(&self) -> u64 {
        self.pos as u64
    }
}

#[derive(Default)]
struct Count {
    models: Cell<usize>,
    checks: Cell<usize>,
}
impl crate::parse::limits::Admission for Count {
    fn model(&self) -> Result<(), Error> {
        self.models.set(self.models.get() + 1);
        Ok(())
    }
    fn check(&self) -> Result<(), Error> {
        self.checks.set(self.checks.get() + 1);
        Ok(())
    }
}

/// Actual future suspension through the same grammar as the synchronous oracle.
/// Completion also checks exactly-once model admission and cancellation checks.
pub(crate) fn compare(
    header: &Header,
    declared: crate::parse::header::Features,
    data: &[u8],
    step: usize,
) {
    let _ = parse(header, declared, data, step);
}

fn parse(
    header: &Header,
    declared: crate::parse::header::Features,
    data: &[u8],
    step: usize,
) -> Result<(Head, usize), Error> {
    let mut r = ByteReader::new(data);
    let expected_count = Count::default();
    let expected =
        read_head(header, declared, &mut r, &expected_count).map(|h| (r.pos as u64, h.units()));
    let available = Cell::new(0);
    let eof = Cell::new(false);
    let count = Count::default();
    let mut input = Feed {
        data,
        available: &available,
        eof: &eof,
        pos: 0,
    };
    let mut future = core::pin::pin!(async {
        let h = read_head_async(header, declared, &mut input, &count).await?;
        Ok((h, input.pos() as usize))
    });
    let mut cx = Context::from_waker(Waker::noop());
    let actual: Result<_, Error> = loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => break value,
            Poll::Pending => {
                assert!(!eof.get(), "explicit EOF must complete, not wait forever");
                let before = (count.models.get(), count.checks.get());
                for _ in 0..3 {
                    assert!(future.as_mut().poll(&mut cx).is_pending());
                    assert_eq!((count.models.get(), count.checks.get()), before);
                }
                if available.get() == data.len() {
                    eof.set(true);
                } else {
                    available.set((available.get() + step).min(data.len()));
                }
            }
        }
    };
    // UnitMap is intentionally opaque: compare its full record geometry via
    // Debug output while retaining exact errors and consumed-byte positions.
    assert_eq!(
        alloc::format!(
            "{:?}",
            actual.as_ref().map(|(h, at)| (*at as u64, h.units()))
        ),
        alloc::format!("{expected:?}")
    );
    assert_eq!(count.models.get(), expected_count.models.get());
    assert_eq!(count.checks.get(), expected_count.checks.get());
    actual
}

#[cfg(feature = "std")]
pub(crate) fn decoded_pixels(file: &[u8]) {
    crate::read::file_from_fragments(file);
    let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(file)).unwrap();
    let data = crate::container::parse(file, false, &crate::v1::payload_len)
        .unwrap()
        .data;
    let expected = crate::decode(file).unwrap().1;
    for step in [1, 7, 64] {
        let (head, at) = parse(&hp.header, hp.declared, data, step).unwrap();
        let actual = crate::read::rows_from_fragmented_head(data, head, at, hp.header.version);
        assert_eq!(
            actual, expected,
            "fragmented head changes reconstructed pixels"
        );
        for open in [false, true] {
            let (head, at) =
                crate::read::head_from_fragments(&hp.header, hp.declared, data, step, open)
                    .unwrap();
            assert_eq!(
                crate::read::rows_from_fragmented_head(data, head, at, hp.header.version),
                expected,
                "owned fragmented transport changes reconstructed pixels"
            );
        }
    }
}

#[test]
fn fragmented_head_varints_keep_partial_bytes() {
    let cases: &[&[u8]] = &[
        &[],
        &[0],
        &[127],
        &[128],
        &[128, 0],
        &[128, 1],
        &[255, 255, 0],
        &[255, 255, 255, 255, 255, 255, 255, 255, 255, 1],
        &[255, 255, 255, 255, 255, 255, 255, 255, 255, 2],
        &[128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
    ];
    for data in cases {
        let expected = ByteReader::new(data).varint();
        let available = Cell::new(0);
        let eof = Cell::new(false);
        let mut input = Feed {
            data,
            available: &available,
            eof: &eof,
            pos: 0,
        };
        let mut future = core::pin::pin!(input.varint());
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(actual) = future.as_mut().poll(&mut cx) {
                assert_eq!(actual, expected);
                break;
            }
            assert!(!eof.get());
            if available.get() == data.len() {
                eof.set(true);
            } else {
                available.set(available.get() + 1);
            }
        }
    }
}
