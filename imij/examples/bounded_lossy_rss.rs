//! One lossy encode per process, for external peak-memory accounting
//! (`/usr/bin/time -v`): `bounded_lossy_rss bounded|session|whole WxH QUALITY EFFORT
//! OUT [SCRATCH] [--sample=u8|u16|u32|f32] [--checksum=SCRATCH]
//! [--speed=fast|compact|none] [--exact=RATIO] [--page] [--source=RAW] [--hash]`.
//! `--source` reads a raw raster file of that geometry (tightly packed RGB
//! of the sample type) instead of the generated texture (bounded mode).
//! The RGB source is generated from its byte position (a deterministic 8-bit
//! texture of gradients, value noise and flat white, widened to the sample
//! type: v * 257 for u16, v * 0x0101_0101 for u32, v / 255 for f32), so the
//! bounded mode never holds a raster; `whole` materializes it for
//! `encode_lossy`. Fast tier by default, one thread. `--checksum` writes a
//! file checksum (bounded: digests spooled to that scratch file); `session`
//! runs `BoundedLossySession` with its I/O done by seek, write and read on
//! the same files (generated source only); `--exact`
//! sets `exact_blocks`; `--page` swaps in a screen-like texture (flat
//! panels, text-like strokes, noise) where exact blocks qualify. Prints the
//! file size and the decoded-pixel XXH3-64 with `--hash`.
use imij::stream_encode::{BoundedLossyConfig, LossyUnitIndex, encode_lossy_bounded};
use imij::{Channels, DecodeSpeed, ImageInfo, Metadata, SampleType};
use std::io::{BufWriter, Read, Seek, SeekFrom};

/// The screen-like texture: flat panels, text-like strokes on light
/// panels, value noise and gradients, in 23 x 17 cells.
fn page(w: u64, at: u64) -> u8 {
    let (p, c) = (at / 3, at % 3);
    let (x, y) = (p % w, p / w);
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ c;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    match (x / 23 + y / 17) % 5 {
        0 => 240,
        1 if (x + y) % 3 == 0 => 20,
        1 => 235,
        2 => (h % 256) as u8,
        3 => ((x * 3 + y * 2 + c * 50) % 256) as u8,
        _ => ((((x ^ y) * 7 + c * 31) % 256) & !63) as u8,
    }
}

fn sample(w: u64, at: u64) -> u8 {
    if PAGE.load(std::sync::atomic::Ordering::Relaxed) {
        return page(w, at);
    }
    let (p, c) = (at / 3, at % 3);
    let (x, y) = (p % w, p / w);
    if (x / 211 + y / 157) % 7 == 0 {
        return 255;
    }
    let mut h = (x / 4).wrapping_mul(0x9E37_79B9) ^ (y / 4).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    ((x + 2 * y + 60 * c) % 256 / 2 + (h % 96)) as u8
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

static PAGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Byte `at` of the raster of `bps`-byte samples (native endian).
fn byte(w: u64, bps: u64, float: bool, at: u64) -> u8 {
    let v = sample(w, at / bps);
    let k = (at % bps) as usize;
    match (bps, float) {
        (1, _) => v,
        (2, _) => (u16::from(v) * 257).to_ne_bytes()[k],
        (4, false) => (u32::from(v) * 0x0101_0101).to_ne_bytes()[k],
        _ => (f32::from(v) / 255.0).to_ne_bytes()[k],
    }
}

/// The generated raster behind `Read + Seek`.
struct Source {
    w: u64,
    len: u64,
    at: u64,
    bps: u64,
    float: bool,
}
impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min((self.len - self.at) as usize);
        for (i, b) in buf[..n].iter_mut().enumerate() {
            *b = byte(self.w, self.bps, self.float, self.at + i as u64);
        }
        self.at += n as u64;
        Ok(n)
    }
}
impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.at = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(d) => self.at.checked_add_signed(d).unwrap(),
            SeekFrom::End(d) => self.len.checked_add_signed(d).unwrap(),
        };
        Ok(self.at)
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (w, h) = a[1].split_once('x').expect("WxH");
    let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
    let opt = |k: &str| a.iter().find_map(|s| s.strip_prefix(k));
    let sample_type = match opt("--sample=").unwrap_or("u8") {
        "u8" => SampleType::U8,
        "u16" => SampleType::U16,
        "u32" => SampleType::U32,
        "f32" => SampleType::F32,
        s => panic!("sample {s}"),
    };
    let checksum = opt("--checksum=");
    PAGE.store(
        a.iter().any(|s| s == "--page"),
        std::sync::atomic::Ordering::Relaxed,
    );
    let info = ImageInfo::new(w, h, Channels::Rgb, sample_type);
    let mut cfg = imij::ImiEncoderConfig::lossy(a[2].parse::<f32>().unwrap())
        .with_effort(a[3].parse().unwrap())
        .with_threads(1)
        .with_checksum(checksum.is_some());
    match opt("--speed=").unwrap_or("fast") {
        "fast" => cfg = cfg.with_decode_speed(DecodeSpeed::Fast),
        "compact" => cfg = cfg.with_decode_speed(DecodeSpeed::Compact),
        "none" => {}
        s => panic!("speed {s}"),
    }
    cfg = cfg.tune_lossy(|t| {
        t.exact_blocks = opt("--exact=").map(|k| k.parse().unwrap());
    });
    let bps = sample_type.bytes() as u64;
    let float = sample_type == SampleType::F32;
    let row = u64::from(w) * 3 * bps;
    let len = row * u64::from(h);
    let open_rw = |p: &str| {
        std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(p)
            .unwrap()
    };
    let out = std::path::Path::new(&a[4]);
    match a[0].as_str() {
        "bounded" => {
            let file = std::fs::File::create(out).unwrap();
            let scratch = open_rw(&a[5]);
            let pulse = how_far_along::FnPulse::new("bounded", |_| Ok(()));
            let mut source: Box<dyn ReadSeek> = match opt("--source=") {
                Some(path) => {
                    let f = std::fs::File::open(path).unwrap();
                    assert_eq!(f.metadata().unwrap().len(), len, "raw source size");
                    Box::new(f)
                }
                None => Box::new(Source {
                    w: w.into(),
                    len,
                    at: 0,
                    bps,
                    float,
                }),
            };
            encode_lossy_bounded(
                &mut source,
                BufWriter::new(file),
                scratch,
                checksum.map(open_rw),
                info.clone(),
                &Metadata::default(),
                &cfg,
                BoundedLossyConfig::new(row, LossyUnitIndex::Reserved(4), 1 << 32),
                &pulse,
            )
            .unwrap();
        }
        "session" => {
            // The sans-IO session, its I/O done here with positioned file
            // reads and writes (the generated source only).
            use imij::stream_encode::{BoundedLossySession, BoundedLossyStep, BoundedStream};
            use std::io::Write;
            let mut file = open_rw(out.to_str().unwrap());
            let mut scratch = open_rw(&a[5]);
            let mut sums = checksum.map(open_rw);
            let mut source = Source {
                w: w.into(),
                len,
                at: 0,
                bps,
                float,
            };
            let mut s = BoundedLossySession::new(
                info.clone(),
                &Metadata::default(),
                &cfg,
                LossyUnitIndex::Reserved(4),
                1 << 32,
            )
            .unwrap();
            // First-output latency: when the first file byte is handed out.
            let start = std::time::Instant::now();
            let mut first_output = None;
            loop {
                match s.step().unwrap() {
                    BoundedLossyStep::Rows { y0, rows, .. } => {
                        source.seek(SeekFrom::Start(u64::from(y0) * row)).unwrap();
                        source.read_exact(rows).unwrap();
                    }
                    BoundedLossyStep::Write {
                        stream,
                        offset,
                        bytes,
                    } => {
                        if stream == BoundedStream::Output && first_output.is_none() {
                            first_output = Some(start.elapsed());
                        }
                        let f = match stream {
                            BoundedStream::Output => &mut file,
                            BoundedStream::IndexScratch => &mut scratch,
                            _ => sums.as_mut().unwrap(),
                        };
                        f.seek(SeekFrom::Start(offset)).unwrap();
                        f.write_all(bytes).unwrap();
                    }
                    BoundedLossyStep::Read {
                        stream,
                        offset,
                        buf,
                    } => {
                        let f = match stream {
                            BoundedStream::IndexScratch => &mut scratch,
                            _ => sums.as_mut().unwrap(),
                        };
                        f.seek(SeekFrom::Start(offset)).unwrap();
                        f.read_exact(buf).unwrap();
                    }
                    BoundedLossyStep::Done { len } => {
                        file.set_len(len).unwrap();
                        eprintln!(
                            "first output after {:.3} s of {:.3} s",
                            first_output.unwrap().as_secs_f64(),
                            start.elapsed().as_secs_f64()
                        );
                        break;
                    }
                    _ => unreachable!(),
                }
            }
        }
        "whole" => {
            let mut px = vec![0u8; len as usize];
            Source {
                w: w.into(),
                len,
                at: 0,
                bps,
                float,
            }
            .read_exact(&mut px)
            .unwrap();
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, row as usize, &info),
                    &Metadata::default(),
                )
                .unwrap();
            std::fs::write(out, file).unwrap();
        }
        m => panic!("mode {m}"),
    }
    let size = std::fs::metadata(out).unwrap().len();
    if a.iter().any(|s| s == "--hash") {
        let file = std::fs::read(out).unwrap();
        let (_, px) = imij::decode(&file).unwrap();
        let mut h = zenhash::Xxh3::new();
        h.update(&px);
        println!("{size} {:016x}", h.digest());
    } else {
        println!("{size}");
    }
}
