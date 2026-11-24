//! Unit table and DATA locator measurements (benchmarks/unit_table_2026-10-08).
//!
//! unit_table_bench sizes OUT.tsv FILES_DIR   table bytes vs the length-prefixed layout
//!                                            (writes both layouts' files to FILES_DIR)
//! unit_table_bench rtt OUT.tsv [MB_PER_S]    region round trips, simulated remote source
//!                                            (100 MB/s per request unless given), and a
//!                                            whole-file read of main's layout as baseline
//! unit_table_bench decode OUT.tsv THREADS    full-decode time of both layouts
//!
//! Inputs are synthetic 8-bit gray (a ramp plus seeded noise, as
//! benchmarks/index_2026-10-08 used); lossy is quality 30. The remote source
//! charges 50 ms per request and 100 MB/s (or MB_PER_S) per request, serving
//! up to C requests of one batch at once (one range per request, as S3 and
//! CDNs serve ranges); its time is simulated, not slept.
use imij::planes::{ReadAt, Rect, SliceReader};
use imij::region::RegionReader;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};
use std::io::Write;
use std::time::Instant;

const LATENCY_MS: f64 = 50.0;

fn gray(side: u32) -> Vec<u8> {
    let mut state = 0x5EEDu32;
    (0..side as usize * side as usize)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let (x, y) = (i % side as usize, i / side as usize);
            ((x / 4 + y / 3 + (state as usize % 17)) & 255) as u8
        })
        .collect()
}

/// ICC 3 KiB, Exif 60 KiB, XMP 200 KiB of deterministic noise.
fn metadata() -> Metadata {
    let mut s = 0x9E37_79B9u32;
    let mut fill = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as u8
            })
            .collect()
    };
    Metadata {
        icc: Some(fill(3 << 10)),
        exif: Some(fill(60 << 10)),
        xmp: Some(fill(200 << 10)),
        ..Default::default()
    }
}

fn encode(side: u32, lossy: bool, meta: &Metadata, table: bool) -> Vec<u8> {
    let info = ImageInfo::new(side, side, Channels::Gray, SampleType::U8);
    let px = gray(side);
    imij::lab::set_length_prefixed_layout(!table);
    let f = if lossy {
        imij::ImiEncoderConfig::lossy(30.0)
            .with_threads(8)
            .encode_samples(imij::ImageRef::new(&px, side as usize, &info), meta)
    } else {
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_threads(8);
            cfg
        };
        cfg.encode_samples(imij::ImageRef::new(&px, side as usize, &info), meta)
    }
    .unwrap();
    imij::lab::set_length_prefixed_layout(false);
    f
}

fn varint(d: &[u8], at: &mut usize) -> u64 {
    let mut v = 0u64;
    for i in 0.. {
        let b = d[*at];
        *at += 1;
        v |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            break;
        }
    }
    v
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// The file without its DATA locator (header extension 4): extension
/// records are the header's last bytes; this masked gray header has the
/// present mask's fields first.
fn without_locator(file: &[u8]) -> Vec<u8> {
    let mut at = 8;
    let n = varint(file, &mut at) as usize;
    let h = &file[at..at + n];
    let body = at + n;
    let mut p = 1;
    let present = varint(h, &mut p);
    let nch = if present & 1 != 0 {
        p += 1;
        h[p - 1] as usize
    } else {
        3
    };
    p += (present >> 1 & 1) as usize + (present >> 2 & 1) as usize * nch;
    p += (present >> 3 & 1) as usize * 4 + (present >> 4 & 1) as usize;
    varint(h, &mut p);
    varint(h, &mut p);
    p += (present >> 5 & 1) as usize + (present >> 6 & 1) as usize + (present >> 7 & 1) as usize;
    let mut head = h[..p].to_vec();
    while p < h.len() {
        let start = p;
        let id = varint(h, &mut p);
        let len = varint(h, &mut p) as usize;
        p += len;
        if id != 4 {
            head.extend_from_slice(&h[start..p]);
        }
    }
    let mut out = file[..8].to_vec();
    put_varint(&mut out, head.len() as u64);
    out.extend_from_slice(&head);
    out.extend_from_slice(&file[body..]);
    out
}

/// Counters of a simulated remote source.
#[derive(Clone, Copy, Default)]
struct Net {
    round_trips: u64,
    requests: u64,
    bytes: u64,
    ms: f64,
}

/// A remote source: simulated round trips, requests, bytes and time.
struct Remote<'a> {
    bytes: &'a [u8],
    concurrency: usize,
    mb_per_s: f64,
    net: std::rc::Rc<std::cell::Cell<Net>>,
}

impl Remote<'_> {
    fn serve(&mut self, offset: u64, out: &mut [u8]) {
        let a = offset as usize;
        out.copy_from_slice(&self.bytes[a..a + out.len()]);
    }
    fn wave(&self, sizes: &[usize]) {
        let mut n = self.net.get();
        n.round_trips += 1;
        n.requests += sizes.len() as u64;
        n.bytes += sizes.iter().sum::<usize>() as u64;
        n.ms += LATENCY_MS + *sizes.iter().max().unwrap() as f64 / (self.mb_per_s * 1e3);
        self.net.set(n);
    }
}

impl ReadAt for Remote<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.serve(offset, out);
        self.wave(&[out.len()]);
        Ok(())
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        // Waves of up to `concurrency` requests; a wave lasts one latency
        // plus its largest transfer.
        for wave in reqs.chunks_mut(self.concurrency) {
            let mut sizes = Vec::with_capacity(wave.len());
            for (offset, out) in wave.iter_mut() {
                self.serve(*offset, out);
                sizes.push(out.len());
            }
            self.wave(&sizes);
        }
        Ok(())
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn sizes(out: &str, dir: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let mut t = std::fs::File::create_new(out).unwrap();
    writeln!(
        t,
        "side\tcoding\tprefixed_bytes\ttable_bytes\tdelta_bytes\tdelta_percent\tlocator_bytes"
    )
    .unwrap();
    for side in [64u32, 128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        for lossy in [false, true] {
            let coding = if lossy { "lossy" } else { "lossless" };
            let none = Metadata::default();
            let (plain, table) = (
                encode(side, lossy, &none, false),
                encode(side, lossy, &none, true),
            );
            std::fs::write(format!("{dir}/prefixed-{side}-{coding}.imi"), &plain).unwrap();
            std::fs::write(format!("{dir}/table-{side}-{coding}.imi"), &table).unwrap();
            let loc = table.len() - without_locator(&table).len();
            let d = table.len() as i64 - plain.len() as i64;
            writeln!(
                t,
                "{side}\t{coding}\t{}\t{}\t{d}\t{:.4}\t{loc}",
                plain.len(),
                table.len(),
                d as f64 * 100.0 / plain.len() as f64
            )
            .unwrap();
            t.flush().unwrap();
            eprintln!("sizes {side} {coding}");
        }
    }
}

fn rtt(out: &str, mb_per_s: f64) {
    let mut t = std::fs::File::create_new(out).unwrap();
    writeln!(t, "side\tcoding\tmetadata\tlayout\tfile_bytes\tconcurrency\topen_round_trips\topen_requests\topen_bytes\tregion_round_trips\tregion_requests\tregion_bytes\tsimulated_ms\tcpu_ms\texact").unwrap();
    let meta = metadata();
    for side in [4096u32, 16384] {
        let px = gray(side);
        let rw = 256u32;
        let rect = Rect::new((side - rw) / 2, (side - rw) / 2, rw, rw);
        let mut want = vec![0u8; (rw * rw) as usize];
        for lossy in [false, true] {
            let coding = if lossy { "lossy" } else { "lossless" };
            for with_meta in [false, true] {
                let m = if with_meta {
                    meta.clone()
                } else {
                    Metadata::default()
                };
                let table = encode(side, lossy, &m, true);
                let layouts = [
                    ("table+locator", table.clone()),
                    ("table", without_locator(&table)),
                    ("prefixed", encode(side, lossy, &m, false)),
                ];
                let full = imij::decode_with_limits(&layouts[0].1, &imij::Limits::NONE)
                    .unwrap()
                    .1;
                for y in 0..rw as usize {
                    let at = (rect.y() as usize + y) * side as usize + rect.x() as usize;
                    want[y * rw as usize..][..rw as usize]
                        .copy_from_slice(&full[at..][..rw as usize]);
                }
                if !lossy {
                    assert_eq!(full, px);
                }
                for (layout, file) in &layouts {
                    // CPU: open and region from memory, median of 5.
                    let mut cpu = Vec::new();
                    let mut got = vec![0u8; want.len()];
                    for _ in 0..5 {
                        let t0 = Instant::now();
                        let mut rd = RegionReader::open(SliceReader::new(file)).unwrap();
                        rd.decode_region(rect, &mut got, rw as usize).unwrap();
                        cpu.push(t0.elapsed().as_secs_f64() * 1e3);
                    }
                    assert_eq!(got, want, "{side} {coding} {layout}");
                    for conc in [1usize, 4] {
                        let net = std::rc::Rc::new(std::cell::Cell::new(Net::default()));
                        let src = Remote {
                            bytes: file,
                            concurrency: conc,
                            mb_per_s,
                            net: net.clone(),
                        };
                        let mut rd = RegionReader::open(src).unwrap();
                        let open = net.get();
                        let mut g = vec![0u8; want.len()];
                        rd.decode_region(rect, &mut g, rw as usize).unwrap();
                        let all = net.get();
                        writeln!(
                            t,
                            "{side}\t{coding}\t{}\t{layout}\t{}\t{conc}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{:.3}\t{}",
                            if with_meta { "icc+exif+xmp" } else { "none" },
                            file.len(),
                            open.round_trips,
                            open.requests,
                            open.bytes,
                            all.round_trips - open.round_trips,
                            all.requests - open.requests,
                            all.bytes - open.bytes,
                            all.ms,
                            median(cpu.clone()),
                            g == want
                        )
                        .unwrap();
                        t.flush().unwrap();
                    }
                    eprintln!("rtt {side} {coding} meta={with_meta} {layout}");
                }
                // Baseline: a reader of main's layout that fetches the whole
                // file (one request, or four ranges in one batch) and decodes
                // the region from memory.
                let main_file = &layouts[2].1;
                for (layout, parts) in [("whole", 1usize), ("whole4", 4)] {
                    for conc in [1usize, 4] {
                        let net = std::rc::Rc::new(std::cell::Cell::new(Net::default()));
                        let mut src = Remote {
                            bytes: main_file,
                            concurrency: conc,
                            mb_per_s,
                            net: net.clone(),
                        };
                        let mut buf = vec![0u8; main_file.len()];
                        let step = main_file.len().div_ceil(parts);
                        let mut reqs: Vec<(u64, &mut [u8])> = buf
                            .chunks_mut(step)
                            .enumerate()
                            .map(|(i, c)| ((i * step) as u64, c))
                            .collect();
                        src.read_many_at(&mut reqs).unwrap();
                        let all = net.get();
                        let t0 = Instant::now();
                        let mut rd = RegionReader::open(SliceReader::new(&buf)).unwrap();
                        let mut g = vec![0u8; want.len()];
                        rd.decode_region(rect, &mut g, rw as usize).unwrap();
                        let cpu = t0.elapsed().as_secs_f64() * 1e3;
                        writeln!(
                            t,
                            "{side}\t{coding}\t{}\t{layout}\t{}\t{conc}\t{}\t{}\t{}\t0\t0\t0\t{:.1}\t{cpu:.3}\t{}",
                            if with_meta { "icc+exif+xmp" } else { "none" },
                            main_file.len(),
                            all.round_trips,
                            all.requests,
                            all.bytes,
                            all.ms,
                            g == want
                        )
                        .unwrap();
                        t.flush().unwrap();
                    }
                }
            }
        }
    }
}

fn decode(out: &str, threads: &str) {
    let threads: Vec<usize> = threads.split(',').map(|s| s.parse().unwrap()).collect();
    let mut t = std::fs::File::create_new(out).unwrap();
    writeln!(
        t,
        "side\tcoding\tlayout\tthreads\tfile_bytes\tdecode_median_ms\truns"
    )
    .unwrap();
    for side in [4096u32, 16384] {
        for lossy in [false, true] {
            let coding = if lossy { "lossy" } else { "lossless" };
            let none = Metadata::default();
            for (layout, table) in [("table", true), ("prefixed", false)] {
                let file = encode(side, lossy, &none, table);
                let mut out = vec![0u8; side as usize * side as usize];
                for &th in &threads {
                    imij::decode_into_threads(&file, &mut out, side as usize, th).unwrap();
                    let runs = 7;
                    let ms: Vec<f64> = (0..runs)
                        .map(|_| {
                            let t0 = Instant::now();
                            imij::decode_into_threads(&file, &mut out, side as usize, th).unwrap();
                            t0.elapsed().as_secs_f64() * 1e3
                        })
                        .collect();
                    writeln!(
                        t,
                        "{side}\t{coding}\t{layout}\t{th}\t{}\t{:.3}\t{runs}",
                        file.len(),
                        median(ms)
                    )
                    .unwrap();
                    t.flush().unwrap();
                }
                eprintln!("decode {side} {coding} {layout}");
            }
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "sizes" => sizes(&a[2], &a[3]),
        "rtt" => rtt(&a[2], a.get(3).map_or(100.0, |m| m.parse().unwrap())),
        "decode" => decode(&a[2], &a[3]),
        m => panic!("unknown mode {m}"),
    }
}
