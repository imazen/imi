//! `cSUM` file checksum writers (2026-10-07). Pair with `csum_read_20261007`,
//! which also builds on commits without the checksum.
//!
//! `csum_write_20261007 etime REPS IN.ppm OUTDIR`: paired encode timing.
//! Encodes the binary PPM (8-bit RGB) lossless (default `ImiEncoderConfig`) and
//! lossy (`imij::ImiEncoderConfig::lossy(75.0)`), each without and with a file checksum
//! (default pieces), interleaved per repetition. Writes
//! `OUTDIR/{ll,ly}-{off,on}.imi` and prints `arm<TAB>bytes<TAB>min_ns<TAB>median_ns`
//! per arm, then the direct cost of one-shot XXH3-64 over each "off" file.
//!
//! `csum_write_20261007 sizes IN.ppm LABEL`: encoded bytes without and with a
//! checksum, lossless and lossy, with the piece size, width and chunk bytes.
//!
//! `csum_write_20261007 corpus IN.ppm OUTDIR`: files without and with a
//! checksum for every writer (lossless, lossy, previews, animations, pages,
//! mixed pages, a plane set, a gain map, science records on an image and in
//! a plane set), the lossless file with its table moved last and with an
//! unknown algorithm byte, from the PPM's top left 128x96 pixels.

use std::path::Path;
use std::time::Instant;

use imij::planes::{LevelDescriptor, PlaneDescriptor, PlaneRole, PlaneSetBuilder};
use imij::science::{ChannelCalibration, ScienceRecords};
use imij::{
    Channels, Chunk, FrameConfig, FrameDuration, FrameEncoder, GainMap, GainMapParams, ImageInfo,
    ImageRef, Metadata, SampleType,
};

fn read_ppm(path: &str) -> (ImageInfo, Vec<u8>) {
    let d = std::fs::read(path).unwrap();
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while d[at].is_ascii_whitespace() {
            at += 1;
        }
        if d[at] == b'#' {
            while d[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        let start = at;
        while !d[at].is_ascii_whitespace() {
            at += 1;
        }
        fields.push(String::from_utf8_lossy(&d[start..at]).into_owned());
    }
    assert_eq!(fields[0], "P6");
    assert_eq!(fields[3], "255");
    let (w, h): (u32, u32) = (fields[1].parse().unwrap(), fields[2].parse().unwrap());
    let px = d[at + 1..].to_vec();
    assert_eq!(px.len(), (w * h * 3) as usize);
    (ImageInfo::new(w, h, Channels::Rgb, SampleType::U8), px)
}

fn stats(t: &mut [u64]) -> (u64, u64) {
    t.sort_unstable();
    (t[0], t[t.len() / 2])
}

fn etime(reps: usize, info: &ImageInfo, px: &[u8], out: &Path) {
    let meta = Metadata::default();
    let rb = info.row_bytes();
    let ll = |c: bool| {
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_checksum(c);
            cfg
        };
        cfg.encode_samples(imij::ImageRef::new(px, rb, info), &meta)
            .unwrap()
    };
    let ly = |c: bool| {
        imij::ImiEncoderConfig::lossy(75.0)
            .with_checksum(c)
            .encode_samples(imij::ImageRef::new(px, rb, info), &meta)
            .unwrap()
    };
    type Enc<'a> = &'a dyn Fn(bool) -> Vec<u8>;
    let arms: [(&str, Enc, bool); 4] = [
        ("ll-off", &ll, false),
        ("ll-on", &ll, true),
        ("ly-off", &ly, false),
        ("ly-on", &ly, true),
    ];
    let mut times = vec![Vec::with_capacity(reps); arms.len()];
    let mut files = vec![Vec::new(); arms.len()];
    for _ in 0..reps {
        for (i, (_, enc, c)) in arms.iter().enumerate() {
            let t = Instant::now();
            files[i] = enc(*c);
            times[i].push(t.elapsed().as_nanos() as u64);
        }
    }
    for (i, (name, _, _)) in arms.iter().enumerate() {
        std::fs::write(out.join(format!("{name}.imi")), &files[i]).unwrap();
        let (min, med) = stats(&mut times[i]);
        println!("{name}\t{}\t{min}\t{med}", files[i].len());
    }
    // What the writer and readers hash: every byte of the "off" file (the
    // pieces of the "on" file cover the same bytes plus the header record).
    type Hash = fn(&[u8]) -> u128;
    let hashes: [(&str, Hash); 1] = [("hash64", |b| zenhash::xxh3_64(b) as u128)];
    for (name, f) in [("ll", &files[0]), ("ly", &files[2])] {
        let n = (200_000_000 / f.len().max(1)).clamp(20, 20_000);
        for (algo, h) in hashes {
            let mut t = Vec::with_capacity(n);
            let mut sink = 0u128;
            for _ in 0..n {
                let s = Instant::now();
                sink ^= h(std::hint::black_box(f.as_slice()));
                t.push(s.elapsed().as_nanos() as u64);
            }
            let (min, med) = stats(&mut t);
            println!("{name}-{algo}\t{}\t{min}\t{med}\t{}", f.len(), sink & 1);
        }
    }
}

/// The byte range of the `cSUM` chunk.
fn csum_range(f: &[u8]) -> (usize, usize) {
    let varint = |at: &mut usize| {
        let mut v = 0usize;
        for i in 0.. {
            let b = f[*at];
            *at += 1;
            v |= ((b & 0x7f) as usize) << (7 * i);
            if b < 0x80 {
                break;
            }
        }
        v
    };
    let mut at = 8;
    let n = varint(&mut at);
    at += n;
    loop {
        let start = at;
        let tag = &f[at..at + 4];
        at += 4;
        let n = varint(&mut at) - 1;
        at += n;
        if tag == b"cSUM" {
            return (start, at);
        }
    }
}

/// `f` with its `cSUM` chunk moved to the end (still valid: the table
/// covers the file without the chunk, wherever it sits).
fn table_last(f: &[u8]) -> Vec<u8> {
    let (a, b) = csum_range(f);
    [&f[..a], &f[b..], &f[a..b]].concat()
}

fn crop(info: &ImageInfo, px: &[u8], w: u32, h: u32) -> (ImageInfo, Vec<u8>) {
    let (w, h) = (w.min(info.width), h.min(info.height));
    let rb = info.row_bytes();
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&px[y * rb..y * rb + w as usize * 3]);
    }
    (ImageInfo::new(w, h, Channels::Rgb, SampleType::U8), out)
}

fn half(info: &ImageInfo, px: &[u8]) -> (ImageInfo, Vec<u8>) {
    let (w, h) = (info.width / 2, info.height / 2);
    let rb = info.row_bytes();
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..3 {
                let s: u32 = [(0, 0), (0, 1), (1, 0), (1, 1)]
                    .iter()
                    .map(|(dy, dx)| px[(2 * y + dy) * rb + (2 * x + dx) * 3 + c] as u32)
                    .sum();
                out.push(((s + 2) / 4) as u8);
            }
        }
    }
    (ImageInfo::new(w, h, Channels::Rgb, SampleType::U8), out)
}

fn corpus(info: &ImageInfo, px: &[u8], out: &Path) {
    let (info, px) = crop(info, px, 128, 96);
    let rb = info.row_bytes();
    let meta = Metadata {
        icc: Some(b"icc profile bytes for the corpus".repeat(4)),
        exif: Some(b"MM\0*corpus exif".to_vec()),
        xmp: Some(b"<x:xmpmeta/>".to_vec()),
        chunks: vec![Chunk {
            tag: *b"vnDr",
            data: b"vendor".to_vec(),
        }],
        ..Default::default()
    };
    let write = |name: &str, bytes: &[u8]| std::fs::write(out.join(name), bytes).unwrap();
    let variants = [("off", false), ("on", true)];
    let (pinfo, ppx) = half(&info, &px);
    let gain = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&ppx, pinfo.row_bytes(), &pinfo),
            &Metadata::default(),
        )
        .unwrap();
    for (v, c) in variants {
        let ll = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_checksum(c);
            cfg
        };
        let ly = imij::ImiEncoderConfig::lossy(80.0).with_checksum(c);
        let plain = ll
            .encode_samples(imij::ImageRef::new(&px, rb, &info), &meta)
            .unwrap();
        write(&format!("ll-{v}.imi"), &plain);
        if c {
            // The table moved to the end (as a streaming writer puts it), and
            // replaced by one of an algorithm this library does not know.
            write("ll-on-last.imi", &table_last(&plain));
            let (a, b) = csum_range(&plain);
            let mut unknown = plain[..a].to_vec();
            unknown.extend_from_slice(b"cSUM\x15\x07");
            unknown.extend_from_slice(&[0x5a; 19]);
            unknown.extend_from_slice(&plain[b..]);
            write("ll-unknown-algo.imi", &unknown);
        }
        write(
            &format!("ly-{v}.imi"),
            &ly.encode_samples(imij::ImageRef::new(&px, rb, &info), &meta)
                .unwrap(),
        );
        let p = [ImageRef {
            pixels: &ppx,
            stride: pinfo.row_bytes(),
            info: &pinfo,
        }];
        write(
            &format!("prv-{v}.imi"),
            &ll.encode_samples_with_previews(imij::ImageRef::new(&px, rb, &info), &meta, &p)
                .unwrap(),
        );
        for (kind, coding) in [("ll", ll.clone()), ("ly", ly.clone())] {
            let mut e =
                FrameEncoder::animation(info.clone(), meta.clone(), FrameConfig::new(coding));
            for k in 0..4u8 {
                let mut f = px.clone();
                for y in 20..44 {
                    for x in 30..70 {
                        f[y * rb + x * 3 + 1] = f[y * rb + x * 3 + 1].wrapping_add(30 * k);
                    }
                }
                e.push_frame(&f, rb, FrameDuration::from_millis(50))
                    .unwrap();
            }
            write(&format!("anim-{kind}-{v}.imi"), &e.finish().unwrap());
        }
        let mut e = FrameEncoder::pages(
            info.clone(),
            Metadata::default(),
            FrameConfig::new(ll.clone()),
        );
        e.push_page(&px, rb, &info).unwrap();
        e.push_page(&ppx, pinfo.row_bytes(), &pinfo).unwrap();
        write(&format!("pages-{v}.imi"), &e.finish().unwrap());
        // Mixed pages: RGB lossless, 16-bit gray lossless, RGB lossy.
        let mut e = FrameEncoder::mixed_pages(FrameConfig::new(ll.clone()));
        e.push_page_with(&px, rb, &info, &meta, &ll.clone())
            .unwrap();
        let ginfo = ImageInfo::new(pinfo.width, pinfo.height, Channels::Gray, SampleType::U16);
        let gray: Vec<u8> = (0..pinfo.width * pinfo.height)
            .flat_map(|n| ((n * 37) as u16).to_ne_bytes())
            .collect();
        e.push_page_with(
            &gray,
            ginfo.row_bytes(),
            &ginfo,
            &Metadata::default(),
            &ll.clone(),
        )
        .unwrap();
        e.push_page_with(
            &ppx,
            pinfo.row_bytes(),
            &pinfo,
            &Metadata::default(),
            &ly.clone(),
        )
        .unwrap();
        write(&format!("mixed-{v}.imi"), &e.finish().unwrap());
        // Science records on the image, and on a plane of a plane set.
        let mut cal = ChannelCalibration::default();
        cal.slope = 0.25;
        cal.unit = Some("kelvin".into());
        let mut records = ScienceRecords::default();
        records.calibration = vec![cal.clone(); 3];
        let mut smeta = meta.clone();
        records.write_metadata(&info, &mut smeta).unwrap();
        write(
            &format!("science-{v}.imi"),
            &ll.encode_samples(imij::ImageRef::new(&px, rb, &info), &smeta)
                .unwrap(),
        );
        let planes = |science: Option<ScienceRecords>| {
            let mut b = PlaneSetBuilder::new(&plain).unwrap();
            let id = b
                .add_plane(
                    PlaneDescriptor::new("depth", SampleType::U16).with_role(PlaneRole::Depth),
                )
                .unwrap();
            let depth: Vec<u8> = (0..64u16 * 48)
                .flat_map(|n| (n * 13).to_ne_bytes())
                .collect();
            b.add_level(
                id,
                LevelDescriptor::new(64, 48, 32, 16),
                &depth,
                128,
                imij::ImiEncoderConfig::default(),
            )
            .unwrap();
            if let Some(r) = science {
                b.set_science(Some(id), r).unwrap();
            }
            b.finish().unwrap()
        };
        write(&format!("planes-{v}.imi"), &planes(None));
        let mut plane_records = ScienceRecords::default();
        plane_records.calibration.push(cal);
        write(
            &format!("planes-science-{v}.imi"),
            &planes(Some(plane_records)),
        );
        let mut params = GainMapParams::default();
        for ch in &mut params.channels {
            ch.min = 0.0;
            ch.max = 2.0;
        }
        params.alternate_hdr_headroom = 2.0;
        let gm = Metadata {
            gain_map: Some(GainMap {
                params,
                alt_cicp: None,
                alt_icc: None,
                image: gain.clone(),
            }),
            ..meta.clone()
        };
        write(
            &format!("gain-{v}.imi"),
            &ll.encode_samples(imij::ImageRef::new(&px, rb, &info), &gm)
                .unwrap(),
        );
    }
}

/// Encoded sizes without and with a checksum, and the piece size and width
/// the "on" file carries (`IMIJ_CSUM_PIECE_LOG2` / `IMIJ_CSUM_WIDTH` override
/// them in `lab` builds).
fn sizes(name: &str, info: &ImageInfo, px: &[u8]) {
    let rb = info.row_bytes();
    let meta = Metadata::default();
    for (coding, off, on) in [
        (
            "ll",
            imij::ImiEncoderConfig::default()
                .encode_samples(imij::ImageRef::new(px, rb, info), &meta)
                .unwrap(),
            imij::ImiEncoderConfig::default()
                .with_checksum(true)
                .encode_samples(imij::ImageRef::new(px, rb, info), &meta)
                .unwrap(),
        ),
        (
            "ly",
            imij::ImiEncoderConfig::lossy(75.0)
                .encode_samples(imij::ImageRef::new(px, rb, info), &meta)
                .unwrap(),
            imij::ImiEncoderConfig::lossy(75.0)
                .with_checksum(true)
                .encode_samples(imij::ImageRef::new(px, rb, info), &meta)
                .unwrap(),
        ),
    ] {
        let (a, b) = csum_range(&on);
        let mut at = a + 4;
        while on[at] & 0x80 != 0 {
            at += 1;
        }
        let (width, k) = (on[at + 2], on[at + 3]);
        println!(
            "{name}\t{coding}\t{}\t{}\t{k}\t{width}\t{}",
            off.len(),
            on.len(),
            b - a
        );
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("etime | corpus | sizes");
    let reps: usize = if mode == "etime" {
        args.next().expect("REPS").parse().unwrap()
    } else {
        0
    };
    let (info, px) = read_ppm(&args.next().expect("IN.ppm"));
    let out = std::path::PathBuf::from(args.next().expect("OUTDIR (sizes: a label)"));
    if mode != "sizes" {
        std::fs::create_dir_all(&out).unwrap();
    }
    match mode.as_str() {
        "etime" => etime(reps, &info, &px, &out),
        "corpus" => corpus(&info, &px, &out),
        "sizes" => sizes(&out.to_string_lossy(), &info, &px),
        m => panic!("unknown mode {m}"),
    }
}
