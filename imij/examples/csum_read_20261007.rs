//! `cSUM` file checksum checks (2026-10-07) that use only decode APIs older
//! than the checksum, so this source builds unchanged on a commit without it
//! (whose decoders skip `cSUM` as an unknown ancillary chunk).
//!
//! `csum_read_20261007 dtime REPS THREADS FILE...`: paired decode timing
//! (`csum_timing::dtime`) of `decode` (`decode_into`, 1 thread), `threads`
//! (`decode_into_threads` with THREADS), `reader` (`decode_reader` over the
//! in-memory bytes) and `stream` (`StreamDecoder`, row by row).
//!
//! `csum_read_20261007 hash FILE...`: one line per file and read path with
//! an FNV-1a hash of the decoded pixels or the error, for comparing builds:
//! `decode`, `reader`, `stream`, `frames` (every frame in place, every page
//! in its own buffer; mixed pages need a build with them), the tags
//! of the chunks `probe` returns, `PlaneSetReader`'s planes (each first
//! level decoded whole), and the science records (SPEC 1.5) of the image
//! and of every plane. Mixed pages and science records need a build with
//! them (main 2722c0be or later).

#[path = "csum_timing/mod.rs"]
mod csum_timing;
use csum_timing::{DecodePath, dtime, fold, read_files};

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h = (*h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
}

fn show<T>(r: Result<T, imij::Error>, f: impl FnOnce(T) -> String) -> String {
    match r {
        Ok(v) => f(v),
        Err(e) => format!("error: {e}"),
    }
}

fn hash_file(data: &[u8]) -> Vec<(&'static str, String)> {
    const SEED: u64 = 0xcbf2_9ce4_8422_2325;
    let decode = show(imij::decode(data), |(_, px)| {
        let mut h = SEED;
        fnv(&mut h, &px);
        format!("{h:016x}")
    });
    let mut h = SEED;
    let reader = show(
        imij::decode_reader(data, &imij::Limits::DEFAULT, |_, _, row| {
            fnv(&mut h, row);
            Ok(())
        }),
        |_| format!("{h:016x}"),
    );
    let stream = show(
        (|| {
            let mut s = imij::StreamDecoder::new(data)?;
            let mut row = vec![0u8; s.info().row_bytes()];
            let mut h = SEED;
            while s.next_row(&mut row)? {
                fnv(&mut h, &row);
            }
            Ok(h)
        })(),
        |h| format!("{h:016x}"),
    );
    let frames = show(
        (|| {
            let mut d = imij::FrameDecoder::new(data)?;
            let (rb, len) = (d.info().row_bytes(), d.info().checked_buffer_len().unwrap());
            let pages = d.kind() == imij::FrameKind::Pages;
            let mixed = d.kind() == imij::FrameKind::MixedPages;
            let mut canvas = vec![0u8; len];
            let mut h = SEED;
            let mut n = 0;
            while let Some(fh) = d.next_frame()? {
                if mixed {
                    // Each page in its own format and size.
                    let info = d.frame_info().unwrap();
                    let mut page = vec![0u8; info.checked_buffer_len().unwrap()];
                    d.decode_into(&mut page, info.row_bytes(), 1)?;
                    fnv(&mut h, &page);
                } else if pages {
                    let bpp = rb / d.info().width as usize;
                    let stride = fh.width as usize * bpp;
                    let mut page = vec![0u8; stride * fh.height as usize];
                    d.decode_into(&mut page, stride, 1)?;
                    fnv(&mut h, &page);
                } else {
                    d.decode_into(&mut canvas, rb, 1)?;
                    fnv(&mut h, &canvas);
                }
                n += 1;
            }
            Ok((n, h))
        })(),
        |(n, h)| format!("{n} frames {h:016x}"),
    );
    let chunks = show(imij::probe(data), |(_, m)| {
        let tags: Vec<String> = m
            .chunks
            .iter()
            .map(|c| String::from_utf8_lossy(&c.tag).into_owned())
            .collect();
        format!("[{}]", tags.join(","))
    });
    let planes = show(
        (|| {
            let mut r = imij::planes::PlaneSetReader::open(
                imij::planes::SliceReader::new(data),
                imij::planes::PlaneSetLimits::default(),
            )?;
            // Every plane's first level, decoded whole.
            let mut h = SEED;
            for i in 0..r.planes().len() {
                let (w, height, row) = {
                    let p = &r.planes()[i];
                    let l = &p.levels()[0];
                    let row = l.width() as usize * p.sample_type().bytes();
                    (l.width(), l.height(), row)
                };
                let mut out = vec![0u8; row * height as usize];
                let id = r.plane_id(i).unwrap();
                let rect = imij::planes::Rect::new(0, 0, w, height);
                r.decode_region(id, 0, rect, &mut out, row)?;
                fnv(&mut h, &out);
            }
            Ok((r.planes().len(), h))
        })(),
        |(n, h)| format!("{n} planes {h:016x}"),
    );
    let science = show(
        (|| {
            let (info, meta) = imij::probe(data)?;
            let mut h = SEED;
            let image = imij::science::ScienceRecords::from_metadata(&info, &meta)?;
            fnv(&mut h, format!("{image:?}").as_bytes());
            let planes = imij::planes::PlaneSetReader::open(
                imij::planes::SliceReader::new(data),
                imij::planes::PlaneSetLimits::default(),
            );
            let mut n = 0;
            if let Ok(mut r) = planes {
                for i in 0..r.planes().len() {
                    let id = r.plane_id(i).unwrap();
                    fnv(&mut h, format!("{:?}", r.science(id)?).as_bytes());
                    n += 1;
                }
            }
            Ok((image.is_empty(), n, h))
        })(),
        |(empty, n, h)| format!("image records {} planes {n} {h:016x}", !empty),
    );
    vec![
        ("decode", decode),
        ("reader", reader),
        ("stream", stream),
        ("frames", frames),
        ("probe-chunks", chunks),
        ("plane-reader", planes),
        ("science", science),
    ]
}

/// Decode paths with every checksum verified (the default of each API).
const PATHS: [DecodePath; 4] = [
    ("decode", |d, i, out, _| {
        imij::decode_into(d, out, i.row_bytes()).unwrap();
        fold(&out[out.len() / 2..])
    }),
    ("threads", |d, i, out, t| {
        imij::decode_into_threads(d, out, i.row_bytes(), t).unwrap();
        fold(&out[out.len() / 3..])
    }),
    ("reader", |d, _, _, _| {
        let mut s = 0;
        imij::decode_reader(d, &imij::Limits::NONE, |_, _, row| {
            s ^= fold(row);
            Ok(())
        })
        .unwrap();
        s
    }),
    ("stream", |d, i, out, _| {
        let mut s = imij::StreamDecoder::new(d).unwrap();
        let row = &mut out[..i.row_bytes()];
        let mut acc = 0;
        while s.next_row(row).unwrap() {
            acc ^= fold(row);
        }
        acc
    }),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("dtime | hash");
    if mode == "dtime" {
        let reps = args.next().expect("REPS").parse().unwrap();
        let threads = args.next().expect("THREADS").parse().unwrap();
        dtime(&PATHS, &read_files(args), reps, threads);
        return;
    }
    assert_eq!(mode, "hash", "unknown mode {mode}");
    for (name, data) in read_files(args) {
        for (path, result) in hash_file(&data) {
            println!("{name}\t{path}\t{result}");
        }
    }
}
