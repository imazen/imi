//! The cross-route outcome matrix (tests/matrix.rs): every decode route run
//! on one file, each outcome reduced to a short string, and the expectation
//! tables those strings are checked against.
//!
//! An outcome is `ok` (the pixels equal the case's reference), `ok!` (they
//! differ: wrong pixels), `opened` (a frame decoder accepted a file whose
//! canvas is too large to allocate here) or the error (`Corrupt:message`,
//! `Unsupported:message`, `TooLarge`, ...). A row route that delivered a row
//! differing from the reference before it failed gets `+rows!` appended.
//! A panic is `PANIC`, and fails the matrix whatever the tables say.
//!
//! Tables (`tests/matrix/<build>.txt`) hold one line per case: its name,
//! the outcome most routes give, then `route=outcome` for each route that
//! gives another. The exceptions are today's divergences between routes:
//! a parse-layer change that unifies routes removes them, and one that
//! changes behaviour shows up as a changed line. `IMIJ_MATRIX_BLESS=1`
//! rewrites the table of the running build from what it observes.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::planes::{Rect, SeekReader};
use imij::region::{RegionReader, decode_region, decode_region_threads};
use imij::{Error, FrameDecoder, ImageInfo, Limits, StreamDecoder};

/// One file and what decoding it should give.
pub struct Case {
    pub name: String,
    pub file: Vec<u8>,
    /// The unmutated image: output buffers are sized from it, never from a
    /// (possibly damaged) header.
    pub info: ImageInfo,
    /// Pixels of the primary image (tightly packed rows).
    pub reference: Vec<u8>,
    /// Every frame or page, concatenated as the frame routes deliver them
    /// (multi-frame files only).
    pub frames: Option<Vec<u8>>,
    /// Limits the routes that take them use (with the route's own checksum
    /// setting).
    pub limits: Limits,
}

pub enum Out {
    Pixels(Vec<u8>),
    /// A route that returns no pixels (header readers) succeeded.
    Meta,
    /// A frame decoder accepted a file whose canvas this harness will not
    /// allocate.
    Opened,
}

pub struct Run {
    pub result: Result<Out, Error>,
    /// A row route delivered a row that differs from the reference.
    pub wrong_rows: bool,
}

impl Run {
    fn of(result: Result<Out, Error>) -> Run {
        Run {
            result,
            wrong_rows: false,
        }
    }
}

pub type RouteFn = fn(&Case) -> Run;

/// Largest output a route here allocates (a damaged header may claim more).
const MAX_ALLOC: usize = 256 << 20;

pub fn error_str(e: &Error) -> String {
    match e {
        Error::NotImij => "NotImij".into(),
        Error::Corrupt(m) => format!("Corrupt:{m}"),
        Error::InvalidInput(m) => format!("InvalidInput:{m}"),
        Error::TooLarge => "TooLarge".into(),
        Error::Cancelled => "Cancelled".into(),
        Error::Unsupported(m) => format!("Unsupported:{m}"),
        Error::Read => "Read".into(),
        // `Error` is non-exhaustive: a class this table does not know.
        e => format!("{e:?}"),
    }
}

fn limits(case: &Case, verify: bool) -> Limits {
    case.limits.with_checksum_verification(verify)
}

fn buffer(case: &Case) -> Vec<u8> {
    vec![0u8; case.info.checked_buffer_len().unwrap()]
}

/// A route that delivers rows: collects them and notes any that differ
/// from the reference rows.
fn rows(
    case: &Case,
    f: impl FnOnce(&mut dyn FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>) -> Result<(), Error>,
) -> Run {
    let mut out = Vec::new();
    let mut wrong = false;
    let r = f(&mut |info, y, row| {
        let rb = row.len();
        if rb == case.info.row_bytes() && info.width == case.info.width {
            let at = y as usize * rb;
            if case.reference.get(at..at + rb) != Some(row) {
                wrong = true;
            }
        }
        out.extend_from_slice(row);
        Ok(())
    });
    Run {
        result: r.map(|_| Out::Pixels(out)),
        wrong_rows: wrong,
    }
}

fn stream<'a>(case: &'a Case, open: impl FnOnce() -> Result<StreamDecoder<'a>, Error>) -> Run {
    rows(case, |sink| {
        let mut d = open()?;
        let info = d.info().clone();
        if info.row_bytes() > MAX_ALLOC {
            return Err(Error::InvalidInput("harness: row too large"));
        }
        let mut row = vec![0u8; info.row_bytes()];
        let mut y = 0;
        while d.next_row(&mut row)? {
            sink(&info, y, &row)?;
            y += 1;
        }
        Ok(())
    })
}

fn threads(case: &Case, n: usize) -> Run {
    let mut out = buffer(case);
    let r = imij::decode_into_threads(&case.file, &mut out, case.info.row_bytes(), n);
    Run::of(r.map(|_| Out::Pixels(out)))
}

fn threads_limits(case: &Case, verify: bool) -> Run {
    let mut out = buffer(case);
    let r = imij::decode_into_threads_with_limits(
        &case.file,
        &mut out,
        case.info.row_bytes(),
        4,
        &limits(case, verify),
    );
    Run::of(r.map(|_| Out::Pixels(out)))
}

fn reader(case: &Case, verify: bool, chunked: bool) -> Run {
    let l = limits(case, verify);
    rows(case, |sink| {
        let r = if chunked {
            imij::decode_reader(Chunked::new(&case.file), &l, sink)
        } else {
            imij::decode_reader(Cursor::new(&case.file), &l, sink)
        };
        r.map(|_| ())
    })
}

fn session(case: &Case, verify: bool) -> DecodeSession {
    DecodeSession::new(DecodePolicy::default().with_output_limits(limits(case, verify)))
}

fn session_reader(case: &Case, verify: bool) -> Run {
    rows(case, |sink| {
        session(case, verify)
            .decode_reader(Chunked::new(&case.file), sink)
            .map(|_| ())
    })
}

fn session_rows(case: &Case) -> Run {
    let rb = imij::header(&case.file)
        .ok()
        .map(|h| h.info.row_bytes())
        .filter(|&n| n <= MAX_ALLOC)
        .unwrap_or(0)
        .max(case.info.row_bytes());
    let mut row = vec![0u8; rb];
    rows(case, |sink| {
        session(case, true)
            .decode_rows(&case.file, &mut row, sink)
            .map(|_| ())
    })
}

/// Every frame or page, concatenated: an animation's canvas after each
/// frame, each page at its own size.
fn frames<'a>(open: impl FnOnce() -> Result<FrameDecoder<'a>, Error>) -> Run {
    let r = (|| {
        let mut d = open()?;
        let canvas = d.info().checked_buffer_len().unwrap_or(usize::MAX);
        if canvas > MAX_ALLOC {
            return Ok(Out::Opened);
        }
        let mut all = Vec::new();
        let mut buf = vec![0u8; canvas];
        let animation = d.kind() == imij::FrameKind::Animation;
        while let Some(fh) = d.next_frame()? {
            if animation {
                let rb = d.info().row_bytes();
                d.decode_into(&mut buf, rb, 2)?;
                all.extend_from_slice(&buf);
                continue;
            }
            let Some(info) = d.frame_info() else {
                continue;
            };
            let n = info.checked_buffer_len().unwrap_or(usize::MAX);
            if n > MAX_ALLOC {
                return Ok(Out::Opened);
            }
            let mut page = vec![0u8; n];
            d.decode_into(&mut page, info.row_bytes(), 2)?;
            let _ = fh;
            all.extend_from_slice(&page);
        }
        Ok(Out::Pixels(all))
    })();
    Run::of(r)
}

/// The base image's rectangle of the primary image, through a region
/// reader over a `Read + Seek` source, in 2 x 2 tiles.
fn region_tiles(case: &Case, verify: bool) -> Run {
    let r = (|| {
        let src = SeekReader::new(Cursor::new(case.file.as_slice()));
        let mut rr = RegionReader::open_with_limits(src, &limits(case, verify))?;
        let (w, h) = (case.info.width, case.info.height);
        let rb = case.info.row_bytes();
        let bpp = rb / w as usize;
        let mut out = buffer(case);
        let (w0, h0) = (w.div_ceil(2), h.div_ceil(2));
        for (x, tw) in [(0, w0), (w0, w - w0)] {
            for (y, th) in [(0, h0), (h0, h - h0)] {
                if tw == 0 || th == 0 {
                    continue;
                }
                let at = y as usize * rb + x as usize * bpp;
                rr.decode_region(Rect::new(x, y, tw, th), &mut out[at..], rb)?;
            }
        }
        Ok(Out::Pixels(out))
    })();
    Run::of(r)
}

fn region_whole(case: &Case, threads: usize) -> Run {
    let mut out = buffer(case);
    let rect = Rect::new(0, 0, case.info.width, case.info.height);
    let rb = case.info.row_bytes();
    let r = if threads == 1 {
        decode_region(&case.file, rect, &mut out, rb)
    } else {
        decode_region_threads(&case.file, rect, &mut out, rb, threads)
    };
    Run::of(r.map(|_| Out::Pixels(out)))
}

/// A reader delivering 1..=7 bytes per call.
pub struct Chunked<'a> {
    data: &'a [u8],
    calls: usize,
}

impl<'a> Chunked<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Chunked { data, calls: 0 }
    }
}

impl std::io::Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.calls += 1;
        let n = (1 + self.calls % 7).min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

/// Route classes: which reference a route's pixels are compared with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The primary image.
    Primary,
    /// Every frame or page.
    Frames,
    /// No pixels.
    Header,
}

pub struct Route {
    pub name: &'static str,
    pub class: Class,
    pub run: RouteFn,
}

const fn route(name: &'static str, class: Class, run: RouteFn) -> Route {
    Route { name, class, run }
}

/// Every route, in table order. `-off` routes run with checksum
/// verification off; routes that take [`Limits`] use the case's.
pub const ROUTES: &[Route] = &[
    route("decode", Class::Primary, |c| {
        Run::of(imij::decode(&c.file).map(|d| Out::Pixels(d.1)))
    }),
    route("decode_with_limits", Class::Primary, |c| {
        Run::of(imij::decode_with_limits(&c.file, &limits(c, true)).map(|d| Out::Pixels(d.1)))
    }),
    route("decode_with_limits-off", Class::Primary, |c| {
        Run::of(imij::decode_with_limits(&c.file, &limits(c, false)).map(|d| Out::Pixels(d.1)))
    }),
    route("decode_into", Class::Primary, |c| {
        let mut out = buffer(c);
        let r = imij::decode_into(&c.file, &mut out, c.info.row_bytes());
        Run::of(r.map(|_| Out::Pixels(out)))
    }),
    route("threads1", Class::Primary, |c| threads(c, 1)),
    route("threads3", Class::Primary, |c| threads(c, 3)),
    route("threads8", Class::Primary, |c| threads(c, 8)),
    route("threads_limits4", Class::Primary, |c| {
        threads_limits(c, true)
    }),
    route("threads_limits4-off", Class::Primary, |c| {
        threads_limits(c, false)
    }),
    route("stream", Class::Primary, |c| {
        stream(c, || StreamDecoder::new(&c.file))
    }),
    route("stream_limits", Class::Primary, |c| {
        stream(c, || StreamDecoder::with_limits(&c.file, &limits(c, true)))
    }),
    route("stream_limits-off", Class::Primary, |c| {
        stream(c, || StreamDecoder::with_limits(&c.file, &limits(c, false)))
    }),
    route("reader", Class::Primary, |c| reader(c, true, false)),
    route("reader_chunked", Class::Primary, |c| reader(c, true, true)),
    route("reader-off", Class::Primary, |c| reader(c, false, false)),
    route("session_decode", Class::Primary, |c| {
        Run::of(session(c, true).decode(&c.file).map(|d| Out::Pixels(d.1)))
    }),
    route("session_decode-off", Class::Primary, |c| {
        Run::of(session(c, false).decode(&c.file).map(|d| Out::Pixels(d.1)))
    }),
    route("session_decode_into", Class::Primary, |c| {
        let mut out = buffer(c);
        let r = session(c, true).decode_into(&c.file, &mut out, c.info.row_bytes());
        Run::of(r.map(|_| Out::Pixels(out)))
    }),
    route("session_rows", Class::Primary, session_rows),
    route("session_reader", Class::Primary, |c| {
        session_reader(c, true)
    }),
    route("session_reader-off", Class::Primary, |c| {
        session_reader(c, false)
    }),
    route("frames", Class::Frames, |c| {
        frames(|| FrameDecoder::new(&c.file))
    }),
    route("frames_limits", Class::Frames, |c| {
        frames(|| FrameDecoder::with_limits(&c.file, &limits(c, true)))
    }),
    route("frames_limits-off", Class::Frames, |c| {
        frames(|| FrameDecoder::with_limits(&c.file, &limits(c, false)))
    }),
    route("region", Class::Primary, |c| region_whole(c, 1)),
    route("region_threads4", Class::Primary, |c| region_whole(c, 4)),
    route("region_tiles", Class::Primary, |c| region_tiles(c, true)),
    route("region_tiles-off", Class::Primary, |c| {
        region_tiles(c, false)
    }),
    route("header", Class::Header, |c| {
        Run::of(imij::header(&c.file).map(|_| Out::Meta))
    }),
    route("probe", Class::Header, |c| {
        Run::of(imij::probe(&c.file).map(|_| Out::Meta))
    }),
    route("previews", Class::Header, |c| {
        Run::of(imij::previews(&c.file).map(|_| Out::Meta))
    }),
    // Writers that read a finished file: the metadata remux and the plane
    // set builder (its primary).
    route("remux", Class::Header, |c| {
        Run::of(imij::compress_metadata(&c.file, 1 << 20).map(|_| Out::Meta))
    }),
    route("plane_builder", Class::Header, |c| {
        Run::of(imij::planes::PlaneSetBuilder::new(&c.file).map(|_| Out::Meta))
    }),
];

/// The outcome string of `route` on `case`.
pub fn observe(case: &Case, route: &Route) -> String {
    let run = match catch_unwind(AssertUnwindSafe(|| (route.run)(case))) {
        Ok(r) => r,
        Err(_) => return "PANIC".into(),
    };
    let mut s = match &run.result {
        Err(e) => error_str(e),
        Ok(Out::Meta) => "ok".into(),
        Ok(Out::Opened) => "opened".into(),
        Ok(Out::Pixels(px)) => {
            let want = match route.class {
                Class::Frames => case.frames.as_ref().unwrap_or(&case.reference),
                _ => &case.reference,
            };
            if px == want { "ok" } else { "ok!" }.into()
        }
    };
    if run.wrong_rows && run.result.is_err() {
        s.push_str("+rows!");
    }
    s
}

/// One table line: the case's outcome on every route.
pub type Line = BTreeMap<&'static str, String>;

pub fn observe_all(case: &Case) -> Line {
    ROUTES.iter().map(|r| (r.name, observe(case, r))).collect()
}

/// The table line of `case`: its name, the commonest outcome (the earlier
/// route's on a tie), then the routes that differ, in route order.
pub fn format_line(name: &str, line: &Line) -> String {
    let mut count: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (i, r) in ROUTES.iter().enumerate() {
        let e = count.entry(line[r.name].as_str()).or_insert((0, i));
        e.0 += 1;
    }
    let common = count
        .iter()
        .max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.cmp(&a.1.1)))
        .unwrap()
        .0
        .to_string();
    let mut s = format!("{name}\t{common}");
    for r in ROUTES {
        if line[r.name] != common {
            s.push_str(&format!("\t{}={}", r.name, line[r.name]));
        }
    }
    s
}

/// Parses a table: case name to (common outcome, exceptions by route).
pub fn parse_table(text: &str) -> BTreeMap<String, (String, BTreeMap<String, String>)> {
    let mut t = BTreeMap::new();
    for l in text.lines() {
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let mut f = l.split('\t');
        let name = f.next().unwrap().to_string();
        let common = f.next().expect("an outcome").to_string();
        let ex = f
            .map(|e| {
                let (r, o) = e.split_once('=').expect("route=outcome");
                (r.to_string(), o.to_string())
            })
            .collect();
        assert!(
            t.insert(name.clone(), (common, ex)).is_none(),
            "{name} twice"
        );
    }
    t
}

/// What `table` expects of `route` on case `name` (None: no line).
pub fn expected<'t>(
    table: &'t BTreeMap<String, (String, BTreeMap<String, String>)>,
    name: &str,
    route: &str,
) -> Option<&'t str> {
    let (common, ex) = table.get(name)?;
    Some(ex.get(route).unwrap_or(common))
}
