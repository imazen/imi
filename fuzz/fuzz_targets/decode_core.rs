// `decode` target: full single-image decode through every output route the
// public API offers, all under explicit limits.
//
// Routes: allocating (`decode_with_limits`), session allocating
// (`DecodeSession::decode`), padded caller-strided output
// (`decode_into_threads_with_limits` and `DecodeSession::decode_into`),
// row delivery (`StreamDecoder::with_limits`, `DecodeSession::decode_rows`)
// and reader streaming (`DecodeSession::decode_reader` over trickling
// `Read` implementations). When two routes succeed their pixels must be
// byte-identical (zero tolerance), and no route may write into a row's
// padding; both are asserted here so a divergence fails with a minimal
// reproducer, not only in the `differential` target.

include!("common_core.rs");

use imij::decode_resources::DecodeSession;
use imij::{StreamDecoder, decode_into_threads_with_limits, decode_with_limits};

/// Padded stride: the logical row plus 0..=31 bytes from the input.
fn stride_for(data: &[u8], rb: usize) -> usize {
    let pad = data.first().copied().unwrap_or(0) as usize % 32;
    rb + pad
}

/// Threads to request: 1..=4, chosen by an input byte.
fn threads_for(data: &[u8]) -> usize {
    1 + data.get(1).copied().unwrap_or(0) as usize % 4
}

/// Returns chunks of at most `chunk` bytes.
struct Trickle<'a> {
    data: &'a [u8],
    pos: usize,
    chunk: usize,
}

impl std::io::Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf
            .len()
            .min(self.chunk)
            .min(self.data.len().saturating_sub(self.pos));
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

pub fn run(data: &[u8]) {
    let limits = decode_limits();
    let full = decode_with_limits(data, &limits);
    let Ok((info, reference)) = full else {
        // Still exercise the bounded session and its reader on refusals.
        let mut session = DecodeSession::new(decode_policy());
        let _ = session.decode(data);
        let mut session = DecodeSession::new(decode_policy());
        let mut trickle = Trickle {
            data,
            pos: 0,
            chunk: 1 + data.first().copied().unwrap_or(0) as usize % 7,
        };
        let _ = session.decode_reader(&mut trickle, |_, _, _| Ok(()));
        return;
    };
    let rb = info.row_bytes();
    let height = info.height as usize;
    let Some(flat) = rb.checked_mul(height) else {
        return;
    };
    if !len_within_limits(flat) {
        return;
    }
    // Cap the padding so the padded buffer stays within the output limit.
    let max_pad = (MAX_BYTES as usize / height.max(1)).saturating_sub(rb);
    let stride = rb + (stride_for(data, rb) - rb).min(max_pad);
    let Some(padded_len) = stride.checked_mul(height) else {
        return;
    };

    // Session allocating route must agree with the plain one.
    let mut session = DecodeSession::new(decode_policy());
    if let Ok((sinfo, spx)) = session.decode(data) {
        assert_eq!(sinfo, info, "session header differs");
        assert_eq!(spx, reference, "session pixels differ");
    }

    // Memory-amplification route: a budget sized from the input. Refusal is
    // legal (the allowance is a capacity plan, not an allocator cap);
    // success must still match bit for bit.
    let mut amp = DecodeSession::new(amplification_policy(data.len()));
    if let Ok((_, apx)) = amp.decode(data) {
        assert_eq!(apx, reference, "input-sized budget changed the pixels");
    }

    // Padded, threaded caller buffer.
    let pad = 0x5a;
    let mut out = vec![pad; padded_len];
    let threads = threads_for(data);
    if let Ok(tinfo) = decode_into_threads_with_limits(data, &mut out, stride, threads, &limits) {
        assert_eq!(tinfo, info, "threaded header differs");
        assert_eq!(rows_tight(&out, stride, rb, height, pad), reference);
    }

    // Session caller-buffer route.
    let mut session = DecodeSession::new(decode_policy());
    let mut sbuf = vec![pad; padded_len];
    if session.decode_into(data, &mut sbuf, stride).is_ok() {
        assert_eq!(rows_tight(&sbuf, stride, rb, height, pad), reference);
    }

    // Session row delivery.
    let mut session = DecodeSession::new(decode_policy());
    let mut row = vec![0u8; rb];
    let mut y = 0usize;
    let expect_row = |y: usize| {
        reference
            .get(y * rb..y * rb + rb)
            .expect("more rows than the header declares")
    };
    let _ = session.decode_rows(data, &mut row, |_, ry, r| {
        assert_eq!(ry as usize, y, "row out of order");
        assert_eq!(r, expect_row(y), "row {y} differs");
        y += 1;
        Ok(())
    });

    // Stream rows.
    if let Ok(mut stream) = StreamDecoder::with_limits(data, &limits) {
        let mut row = vec![0u8; rb];
        let mut y = 0usize;
        while stream.next_row(&mut row).unwrap_or(false) {
            assert_eq!(row, expect_row(y), "stream row {y}");
            y += 1;
        }
        assert_eq!(y, height, "stream ended early");
    }

    // Reader streaming with a trickling source.
    let mut session = DecodeSession::new(decode_policy());
    let mut trickle = Trickle {
        data,
        pos: 0,
        chunk: 1 + data.get(1).copied().unwrap_or(0) as usize % 7,
    };
    let mut y = 0usize;
    let _ = session.decode_reader(&mut trickle, |_, ry, r| {
        assert_eq!(ry as usize, y, "reader row out of order");
        assert_eq!(r, expect_row(y), "reader row {y} differs");
        y += 1;
        Ok(())
    });
}
