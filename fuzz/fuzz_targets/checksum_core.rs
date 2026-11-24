// `checksum` target: decode with `cSUM` verification through the `ReadAt`
// path, including partial reads.
//
// Routes: [`RegionReader`] over the in-memory [`SliceReader`], over a source
// whose `read_many_at` serves each range in halves and in reverse order, and
// over a source that starts failing mid-decode; session slice decode with
// verification on and off; session reader decode over sources that trickle
// and that fail partway. Outputs must not depend on the source's read
// granularity, and turning verification on must never change pixels.

include!("common_core.rs");

use imij::decode_resources::DecodeSession;
use imij::planes::{ReadAt, Rect, SliceReader};
use imij::region::RegionReader;
use imij::{Error, StreamDecoder};

/// A `ReadAt` whose batch reads are served in halves, in reverse order. Legal
/// per the trait contract (ranges are independent); the decode result must not
/// depend on batch granularity.
struct SplitAt<'a> {
    data: &'a [u8],
}

impl ReadAt for SplitAt<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.data.len() as u64)
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::TooLarge)?;
        let end = start.checked_add(out.len()).ok_or(Error::TooLarge)?;
        out.copy_from_slice(
            self.data
                .get(start..end)
                .ok_or(Error::Corrupt("fuzz read past EOF"))?,
        );
        Ok(())
    }

    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        for (offset, out) in reqs.iter_mut().rev() {
            let mid = out.len() / 2;
            self.read_exact_at(*offset, &mut out[..mid])?;
            self.read_exact_at(*offset + mid as u64, &mut out[mid..])?;
        }
        Ok(())
    }
}

/// A `ReadAt` that fails once `after` reads have been served.
struct FaultAt<'a> {
    data: &'a [u8],
    after: u64,
    reads: u64,
}

impl ReadAt for FaultAt<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.data.len() as u64)
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        if self.reads >= self.after {
            return Err(Error::Read);
        }
        self.reads += 1;
        let start = usize::try_from(offset).map_err(|_| Error::TooLarge)?;
        let end = start.checked_add(out.len()).ok_or(Error::TooLarge)?;
        out.copy_from_slice(
            self.data
                .get(start..end)
                .ok_or(Error::Corrupt("fuzz read past EOF"))?,
        );
        Ok(())
    }
}

/// A `Read` that returns small chunks and can fail once at a chosen position.
struct Trickle<'a> {
    data: &'a [u8],
    pos: usize,
    chunk: usize,
    fail_at: Option<usize>,
}

impl std::io::Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.fail_at == Some(self.pos) {
            return Err(std::io::Error::other("fuzz read fault"));
        }
        let n = buf
            .len()
            .min(self.chunk)
            .min(self.data.len().saturating_sub(self.pos));
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn policy(verify: bool) -> imij::decode_resources::DecodePolicy {
    decode_policy().with_output_limits({
        let mut limits = imij::Limits::DEFAULT;
        limits.max_pixels = MAX_PIXELS;
        limits.max_bytes = MAX_BYTES;
        limits.verify_checksum = verify;
        limits
    })
}

/// A small rectangle for the region routes.
fn rect_from(data: &[u8], width: u32, height: u32) -> Rect {
    let at = data.len().saturating_sub(4);
    let tail = |i: usize| -> u32 {
        data.get(at + i).copied().unwrap_or(0) as u32
            | ((data.get(at + i + 1).copied().unwrap_or(0) as u32) << 8)
    };
    let x = tail(0) % width;
    let y = tail(2) % height;
    let w = 1 + tail(0) % (width - x).min(64);
    let h = 1 + tail(2) % (height - y).min(64);
    Rect::new(x, y, w, h)
}

fn decode_small_region<R: ReadAt>(
    reader: &mut RegionReader<R>,
    data: &[u8],
) -> Result<Vec<u8>, Error> {
    let info = reader.info().clone();
    if info.width == 0 || info.height == 0 {
        return Err(Error::Corrupt("empty image"));
    }
    let rect = rect_from(data, info.width, info.height);
    let bpp = info.bytes_per_pixel();
    let rb = rect.width() as usize * bpp;
    let mut out = vec![0u8; rb * rect.height() as usize];
    reader.decode_region(rect, &mut out, rb)?;
    Ok(out)
}

pub fn run(data: &[u8]) {
    // Session slice decode, verification on and off: when both succeed the
    // pixels must match, and verification never changes them.
    let mut on = DecodeSession::new(policy(true));
    let mut off = DecodeSession::new(policy(false));
    let with_verify = on.decode(data);
    let without = off.decode(data);
    if let (Ok((ia, va)), Ok((ib, vb))) = (&with_verify, &without) {
        assert_eq!(ia, ib, "checksum verification changed the header");
        assert_eq!(va, vb, "checksum verification changed the pixels");
    }

    // Memory-amplification route with verification on: a budget sized from
    // the input. Refusal is legal; success must still match.
    let mut amp = DecodeSession::new(amplification_policy(data.len()));
    if let (Ok((_, apx)), Ok((_, expected))) = (amp.decode(data), &with_verify) {
        assert_eq!(&apx, expected, "input-sized budget changed the pixels");
    }

    // Stream rows with verification on, compared against the verified full
    // decode when both succeed.
    if let Ok((info, reference)) = &with_verify {
        let rb = info.row_bytes();
        let limits = decode_limits();
        if let Ok(mut stream) = StreamDecoder::with_limits(data, &limits) {
            let mut row = vec![0u8; rb];
            let mut y = 0usize;
            while stream.next_row(&mut row).unwrap_or(false) {
                assert_eq!(row, reference[y * rb..y * rb + rb], "stream row {y}");
                y += 1;
            }
            assert_eq!(y, info.height as usize, "stream ended early");
        }
    }

    // Region route over the plain in-memory source: the reference for the
    // other source implementations.
    let limits = decode_limits();
    let plain = (|| -> Result<Vec<u8>, Error> {
        let mut reader = RegionReader::open_with_limits(SliceReader::new(data), &limits)?;
        decode_small_region(&mut reader, data)
    })();

    if let Ok(reference) = &plain {
        // Same region through `SplitAt`: batch granularity must not change
        // the opened state or the pixels.
        let split = (|| -> Result<Vec<u8>, Error> {
            let mut reader = RegionReader::open_with_limits(SplitAt { data }, &limits)?;
            decode_small_region(&mut reader, data)
        })();
        assert_eq!(
            split.as_ref().ok(),
            Some(reference),
            "split-batch ReadAt changed the pixels"
        );

        // A source failing after N reads must error, not panic; if it gets
        // through, the pixels must equal the plain route's.
        let after = 1 + data.get(7).copied().unwrap_or(0) as u64 % 8;
        let faulted = (|| -> Result<Vec<u8>, Error> {
            let mut reader = RegionReader::open_with_limits(
                FaultAt {
                    data,
                    after,
                    reads: 0,
                },
                &limits,
            )?;
            decode_small_region(&mut reader, data)
        })();
        if let Ok(px) = &faulted {
            assert_eq!(px, reference, "faulting ReadAt changed the pixels");
        }
    }

    // Session reader over a trickling source (partial reads), verification on.
    let mut session = DecodeSession::new(policy(true));
    let mut trickle = Trickle {
        data,
        pos: 0,
        chunk: 1 + data.first().copied().unwrap_or(0) as usize % 7,
        fail_at: None,
    };
    let mut rows = Vec::new();
    let outcome = session.decode_reader(&mut trickle, |_, _, row| {
        rows.extend_from_slice(row);
        Ok(())
    });
    if let (Ok((_, expected)), Ok(_)) = (&with_verify, &outcome) {
        assert_eq!(&rows, expected, "trickling reader changed the pixels");
    }

    // A reader that fails partway must return an error, not panic, and any
    // rows it delivered before the fault must equal the verified output.
    let mut session = DecodeSession::new(policy(true));
    let mut fault = Trickle {
        data,
        pos: 0,
        chunk: 3,
        fail_at: Some(1 + data.get(5).copied().unwrap_or(0) as usize % data.len().max(2)),
    };
    let mut rows = Vec::new();
    let _ = session.decode_reader(&mut fault, |_, _, row| {
        rows.extend_from_slice(row);
        Ok(())
    });
    if let Ok((_, expected)) = &with_verify {
        assert_eq!(
            &rows[..rows.len().min(expected.len())],
            &expected[..rows.len().min(expected.len())],
            "rows before a read fault differ"
        );
    }
}
