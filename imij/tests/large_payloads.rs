//! Retained V2/V3 wire grammar controls use the explicit legacy producer.
//! Payloads larger than 4 GiB whose records are small, on every target:
//! unit-table hint entries sum in 64 bits, so 32-bit hosts read them as
//! 64-bit hosts do (review of PR #11, finding 3; `just check-large-payloads-i686`
//! runs this file on i686). The bytes past 4 GiB are zero slack at the end
//! of records (SPEC 2.7: words a record's decode never reads), served
//! without being held in memory.

mod common;

use imij::planes::{ReadAt, Rect};
use imij::region::RegionReader;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

/// A run of a file: bytes, or zeros.
enum Seg {
    Bytes(Vec<u8>),
    Zeros(u64),
}

impl Seg {
    fn len(&self) -> u64 {
        match self {
            Seg::Bytes(b) => b.len() as u64,
            Seg::Zeros(n) => *n,
        }
    }
}

/// A file made of runs, read positionally.
struct Virtual(Vec<Seg>);

impl ReadAt for Virtual {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.0.iter().map(Seg::len).sum())
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let mut seg_at = 0u64;
        let mut done = 0usize;
        for s in &self.0 {
            let l = s.len();
            let pos = offset + done as u64;
            if done < out.len() && pos < seg_at + l {
                let from = pos - seg_at;
                let n = ((l - from) as usize).min(out.len() - done);
                match s {
                    Seg::Bytes(b) => {
                        out[done..done + n].copy_from_slice(&b[from as usize..from as usize + n])
                    }
                    Seg::Zeros(_) => out[done..done + n].fill(0),
                }
                done += n;
            }
            seg_at += l;
        }
        if done == out.len() {
            Ok(())
        } else {
            Err(Error::Corrupt("read past end"))
        }
    }
}

/// The same runs through `std::io::Read`.
struct Stream {
    segs: Vec<Seg>,
    i: usize,
    off: u64,
}

impl std::io::Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while let Some(s) = self.segs.get(self.i) {
            let l = s.len();
            if self.off == l {
                self.i += 1;
                self.off = 0;
                continue;
            }
            let n = ((l - self.off) as usize).min(buf.len());
            match s {
                Seg::Bytes(b) => {
                    let a = self.off as usize;
                    buf[..n].copy_from_slice(&b[a..a + n]);
                }
                Seg::Zeros(_) => buf[..n].fill(0),
            }
            self.off += n as u64;
            return Ok(n);
        }
        Ok(0)
    }
}

/// A gray `w`x`h` lossless file with `stripe_rows`-row stripes, rebuilt with
/// `slack` zero bytes after every record whose index `slack_after` accepts
/// (its unit-table entry grown to match) and without a DATA locator, whose
/// prefix would no longer match. Returns the pixels and the file's runs.
fn with_slack(
    w: u32,
    h: u32,
    stripe_rows: u32,
    slack: u64,
    slack_after: impl Fn(usize) -> bool,
) -> (Vec<u8>, Vec<Seg>) {
    let info = ImageInfo::new(w, h, Channels::Gray, SampleType::U8);
    let px: Vec<u8> = (0..w * h)
        .map(|i| ((i * 7 + i / w * 3) % 251) as u8)
        .collect();
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(stripe_rows);
        });
        cfg
    };
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, w as usize, &info),
            &Metadata::default(),
        )
        .unwrap();
    let recs: Vec<_> = common::extensions(&file)
        .into_iter()
        .filter(|e| e.0 != 4)
        .collect();
    let file = common::with_extensions(&file, &recs);
    let d = common::data_payload(&file);
    let payload = &file[d];
    let p = common::lossless_payload(payload, &info);
    assert!(p.table.is_some(), "a unit table hint");
    let extra = |i: usize| if slack_after(i) { slack } else { 0 };
    // Each record's length grows by its slack (SPEC 2.7: bytes its decode
    // never reads), and so does its hint entry (the whole record).
    let lens: Vec<u64> = p
        .records
        .iter()
        .enumerate()
        .map(|(i, r)| (r.end - r.start) as u64 + extra(i))
        .collect();
    let mut hint = Vec::new();
    for &l in &lens {
        common::put_varint(&mut hint, 1 + common::varint_bytes(l).len() as u64 + l);
    }
    let (list_at, list) = p.payload.list.clone().expect("a head record list");
    assert_eq!(p.payload.head_records.len(), 1, "the hint only");
    let mut records = Vec::new();
    common::put_varint(&mut records, common::H_UNIT_TABLE);
    common::put_varint(&mut records, hint.len() as u64);
    records.extend_from_slice(&hint);
    let mut head = payload[..list_at].to_vec();
    common::put_varint(&mut head, records.len() as u64);
    head.extend_from_slice(&records);
    head.extend_from_slice(&payload[list.end..p.payload.head_end]);
    let mut segs = Vec::new();
    let mut total = head.len() as u64;
    let mut run = head;
    for (i, r) in p.records.iter().enumerate() {
        let mut framed = vec![59];
        common::put_varint(&mut framed, lens[i]);
        framed.extend_from_slice(&payload[r.clone()]);
        total += framed.len() as u64 + extra(i);
        run.extend_from_slice(&framed);
        if extra(i) > 0 {
            segs.push(Seg::Bytes(std::mem::take(&mut run)));
            segs.push(Seg::Zeros(extra(i)));
        }
    }
    if !run.is_empty() {
        segs.push(Seg::Bytes(run));
    }
    let (_, data_tag, _) = common::chunks(&file)
        .into_iter()
        .find(|c| &c.0 == b"DATA")
        .unwrap();
    let mut start = file[..data_tag].to_vec();
    start.extend_from_slice(b"DATA");
    common::put_varint(&mut start, total + 1);
    segs.insert(0, Seg::Bytes(start));
    (px, segs)
}

/// The region reader over a 6 GiB file of three units (3 GiB of slack after
/// records 1 and 2): it opens, and the first unit decodes.
#[test]
fn regions_of_a_payload_over_4_gib() {
    let (px, segs) = with_slack(64, 48, 16, 3 << 30, |i| i > 0);
    let mut v = Virtual(segs);
    assert!(v.len().unwrap() > 6 << 30);
    let mut rd = RegionReader::open(v).unwrap();
    assert!(rd.has_unit_table());
    let mut out = vec![0u8; 64 * 16];
    rd.decode_region(Rect::new(0, 0, 64, 16), &mut out, 64)
        .unwrap();
    assert_eq!(out[..], px[..64 * 16]);
}

/// `decode_reader` over a 5.4 GB stream: 1,024 one-row units, each record
/// followed by 5 MiB of slack.
#[test]
fn decode_reader_of_a_payload_over_4_gib() {
    let (px, segs) = with_slack(64, 1024, 1, 5 << 20, |_| true);
    assert!(segs.iter().map(Seg::len).sum::<u64>() > 5 << 30);
    let mut rows = Vec::new();
    imij::decode_reader(
        Stream { segs, i: 0, off: 0 },
        &imij::Limits::DEFAULT,
        |_, _, row| {
            rows.extend_from_slice(row);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(rows, px);
}
