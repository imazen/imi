//! Cross-route outcome matrix: every case (valid files of each family, and
//! structural mutations of them) through every decode route
//! (`common::matrix::ROUTES`), checked against the table of the running
//! build in `tests/matrix/`. The tables record today's behaviour, including
//! where routes disagree; a change that moves an outcome changes a line.
//!
//! `IMIJ_MATRIX_BLESS=1` rewrites the running build's table instead of
//! checking it (`default.txt` in a default build; `lab.txt` and
//! `minimal.txt` hold only the lines where the lab and minimal builds differ
//! from `default.txt`, and `ptr32.txt` likewise only the lines where a
//! 32-bit build differs from its own build's 64-bit tables). Panics, and
//! wrong pixels (`ok!`, `+rows!`) from a route that verifies checksums,
//! fail whatever the table says.
// Literal V2/V3 wire mutation controls select the retained grammar explicitly.

mod common;

use std::collections::BTreeMap;

use common::feature::{
    BILEVEL, CHANNEL_LAYOUTS, LOSSY, MIXED_PAGES, NEAR_LOSSLESS, SAMPLES_32, TEMPLATE_PALETTES,
};
use common::matrix::{Case, Class, ROUTES, expected, format_line, observe_all, parse_table};
use common::{
    chunks, data_payload, extensions, header_record, put_varint, varint, with_extensions,
    with_feature_mask,
};
use imij::{
    Channels, Chunk, Cicp, DecodeSpeed, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder,
    ImageInfo, ImiEncoderConfig, Limits, LossyEntropy, Metadata, SampleType,
};

const DEFAULT_TABLE: &str = include_str!("matrix/default.txt");
const LAB_TABLE: &str = include_str!("matrix/lab.txt");
const MINIMAL_TABLE: &str = include_str!("matrix/minimal.txt");
/// The overlay for 32-bit targets: only the lines whose outcomes differ
/// from what the same build reports on a 64-bit host (a decode route's
/// honest `TooLarge` where the 64-bit tables reach a structural error).
#[cfg(target_pointer_width = "32")]
const PTR32_TABLE: &str = include_str!("matrix/ptr32.txt");
#[cfg(not(target_pointer_width = "32"))]
const PTR32_TABLE: &str = "";

/// The build this test runs in, and its table file.
fn build() -> &'static str {
    if cfg!(feature = "lab") {
        "lab"
    } else if !cfg!(feature = "extended") {
        "minimal"
    } else {
        "default"
    }
}

// ---- pixels and encoders -----------------------------------------------------

/// Deterministic content: gradients, noise and flat patches.
fn pixels(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let nch = info.channels.count();
    let bps = info.sample.bytes();
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let mut out = Vec::with_capacity(w * h * nch * bps);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let flat = (x / 13 + y / 11) % 3 == 0;
                let v: u32 = if flat {
                    (c as u32 * 60 + 40) << 8
                } else {
                    ((x * 211 + y * 97 + c * 50) as u32 % 50_000) + (s % 2048)
                };
                match info.sample {
                    SampleType::U8 | SampleType::I8 => out.push((v >> 8) as u8),
                    SampleType::U16 | SampleType::I16 => {
                        out.extend_from_slice(&(v as u16).to_ne_bytes())
                    }
                    SampleType::F16 => {
                        let half = 0x2400u16 + (v % 0x1800) as u16;
                        out.extend_from_slice(&half.to_ne_bytes());
                    }
                    SampleType::F32 => out.extend_from_slice(&(v as f32 / 65_536.0).to_ne_bytes()),
                    SampleType::F64 => out.extend_from_slice(&(v as f64 / 65_536.0).to_ne_bytes()),
                    SampleType::U64 | SampleType::I64 => out.extend_from_slice(
                        &(v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).to_ne_bytes(),
                    ),
                    _ => out.extend_from_slice(&v.wrapping_mul(40_503).to_ne_bytes()),
                }
            }
        }
    }
    out
}

/// f32 samples whose low 12 mantissa bits are zero (the `FloatShift` map).
fn rounded_f32(info: &ImageInfo, seed: u32) -> Vec<u8> {
    pixels(info, seed)
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|b| (u32::from_ne_bytes(*b) & !0xfff).to_ne_bytes())
        .collect()
}

/// A smooth index field through a few hundred scattered values per coded
/// word (both words of 64-bit samples): value-table maps (SPEC 2.1 map 9).
fn sparse_values(info: &ImageInfo) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let nch = info.channels.count();
    let bytes = info.sample.bytes();
    let depth = 8 * bytes.min(4) as u32;
    let count = if bytes == 1 { 40 } else { 300 };
    let set = |c: usize| -> Vec<u32> {
        let mut s = 0x9e37_79b9u32 ^ (c as u32).wrapping_mul(0x85eb_ca6b);
        let mut v: Vec<u32> = (0..count * 2)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                if depth == 32 {
                    s
                } else {
                    s & ((1 << depth) - 1)
                }
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v.truncate(count);
        v
    };
    let sets: Vec<Vec<u32>> = (0..2 * nch).map(set).collect();
    let mut out = Vec::with_capacity(info.row_bytes() * h);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                let word = |k: usize| {
                    let set = &sets[k * nch + c];
                    let f = (x * 5 + y * 3 + c * 17) % (2 * set.len());
                    set[if f < set.len() {
                        f
                    } else {
                        2 * set.len() - 1 - f
                    }]
                };
                match bytes {
                    1 => out.push(word(0) as u8),
                    2 => out.extend_from_slice(&(word(0) as u16).to_ne_bytes()),
                    4 => out.extend_from_slice(&word(0).to_ne_bytes()),
                    _ => out.extend_from_slice(
                        &((word(0) as u64) << 32 | word(1) as u64).to_ne_bytes(),
                    ),
                }
            }
        }
    }
    out
}

/// At most 6 distinct colors: palette stripes.
fn few_colors(info: &ImageInfo) -> Vec<u8> {
    let n = info.channels.count();
    let (w, h) = (info.width as usize, info.height as usize);
    (0..w * h)
        .flat_map(|i| {
            let k = ((i % w) / 7 + (i / w) / 5) % 6;
            (0..n).map(move |c| (k * 40 + c * 17) as u8)
        })
        .collect()
}

/// A 1-bit page: lines of random stroke glyphs on white (0 = ink, 255).
/// A map-like gray image of nine values in blocks with ragged edges, specks
/// and repeated rows; the rows from `two` on use only two values.
fn map_page(info: &ImageInfo, two: usize) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let mut s = 0x2545_F491u32;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    let mut px = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let r = next() as usize;
            let mut c = ((x + r % 3) / 9 + 3 * ((y + (r >> 9) % 2) / 7)) % 9;
            if r.is_multiple_of(41) {
                c = (r >> 20) % 9;
            }
            if y >= two {
                c = 1 + (x / 5 + y / 3) % 2;
            }
            px[y * w + x] = (c * 23 + 7) as u8;
        }
        if y > 0 && next().is_multiple_of(4) {
            px.copy_within((y - 1) * w..y * w, y * w);
        }
    }
    px
}

fn glyph_page(info: &ImageInfo) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let mut s = 0x9E37_79B9u32;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    let mut px = vec![255u8; w * h];
    for y0 in (3..h - 9).step_by(12) {
        for x0 in (4..w - 7).step_by(7) {
            for _ in 0..3 {
                let (v, at, len) = (
                    next().is_multiple_of(2),
                    next() as usize % 5,
                    3 + next() as usize % 5,
                );
                for k in 0..len.min(if v { 8 } else { 5 }) {
                    let (x, y) = if v {
                        (x0 + at, y0 + k)
                    } else {
                        (x0 + k, y0 + at)
                    };
                    px[y * w + x] = 0;
                }
            }
        }
    }
    px
}

fn metadata() -> Metadata {
    Metadata {
        icc: Some(b"icc profile bytes ".repeat(40)),
        exif: Some(b"MM\0*exif payload".to_vec()),
        xmp: Some(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf/></x:xmpmeta>".repeat(30)),
        chunks: vec![Chunk {
            tag: *b"vnDr",
            data: b"opaque vendor payload".to_vec(),
        }],
        ..Default::default()
    }
}

fn lossless_cfg() -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.tune_lossless(|t| {
        t.stripe_rows = Some(16);
    });
    cfg
}

fn enc(px: &[u8], info: &ImageInfo, meta: &Metadata, cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_legacy_samples(imij::ImageRef::new(px, info.row_bytes(), info), meta)
        .unwrap()
}

fn enc_lossy(px: &[u8], info: &ImageInfo, meta: &Metadata, cfg: &ImiEncoderConfig) -> Vec<u8> {
    cfg.encode_legacy_samples(imij::ImageRef::new(px, info.row_bytes(), info), meta)
        .unwrap()
}

fn lossy_cfg() -> ImiEncoderConfig {
    imij::ImiEncoderConfig::lossy(70.0).tune_lossy(|t| t.stripe_block_rows = 1)
}

// ---- file surgery --------------------------------------------------------------

fn chunk(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut c = tag.to_vec();
    put_varint(&mut c, payload.len() as u64 + 1);
    c.extend_from_slice(payload);
    c
}

fn splice(file: &[u8], at: usize, bytes: &[u8]) -> Vec<u8> {
    [&file[..at], bytes, &file[at..]].concat()
}

/// (start, end) of the first chunk tagged `tag`.
fn find(file: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    chunks(file)
        .into_iter()
        .find(|c| &c.0 == tag)
        .map(|c| (c.1, c.2))
}

fn data_start(file: &[u8]) -> usize {
    find(file, b"DATA").unwrap().0
}

fn data_end(file: &[u8]) -> usize {
    find(file, b"DATA").unwrap().1
}

fn before_data(file: &[u8], c: &[u8]) -> Vec<u8> {
    splice(file, data_start(file), c)
}

fn after_data(file: &[u8], c: &[u8]) -> Vec<u8> {
    splice(file, data_end(file), c)
}

fn without(file: &[u8], tag: &[u8; 4]) -> Vec<u8> {
    let (a, b) = find(file, tag).unwrap();
    [&file[..a], &file[b..]].concat()
}

/// `file` with the first `fRAM`'s presence varint (after x0, y0 and the
/// duration) replaced by `controls`.
fn with_frame_controls(file: &[u8], controls: &[u8]) -> Vec<u8> {
    let (a, b) = find(file, b"fRAM").unwrap();
    let (n, k) = common::varint(&file[a + 4..]);
    let p = &file[a + 4 + k..a + 4 + k + n as usize - 1];
    let mut start = 0;
    for _ in 0..4 {
        common::read_varint(p, &mut start);
    }
    let mut end = start;
    common::read_varint(p, &mut end);
    let mut payload = p[..start].to_vec();
    payload.extend_from_slice(controls);
    payload.extend_from_slice(&p[end..]);
    [&file[..a], &chunk(b"fRAM", &payload), &file[b..]].concat()
}

/// `file` with its `DATA` payload replaced by `payload` (length field
/// rewritten).
fn with_data(file: &[u8], payload: &[u8]) -> Vec<u8> {
    let (a, b) = find(file, b"DATA").unwrap();
    [&file[..a], &chunk(b"DATA", payload), &file[b..]].concat()
}

/// `file` with the `DATA` chunk's length field 0 (open: its payload delimits
/// itself).
fn open_data(file: &[u8]) -> Vec<u8> {
    let (a, b) = find(file, b"DATA").unwrap();
    let p = data_payload(file);
    let mut c = b"DATA".to_vec();
    c.push(0);
    c.extend_from_slice(&file[p.clone()]);
    [&file[..a], &c, &file[b..]].concat()
}

/// `file` cut after an open `DATA` chunk (length field 0) holding `payload`.
fn open_data_at_end(file: &[u8], payload: &[u8]) -> Vec<u8> {
    let (a, _) = find(file, b"DATA").unwrap();
    [&file[..a], &b"DATA\0"[..], payload].concat()
}

/// The header record byte at `i` (0: the version) set to `v`.
fn with_header_byte(file: &[u8], i: usize, v: u8) -> Vec<u8> {
    let mut f = file.to_vec();
    f[header_record(file).start + i] = v;
    f
}

fn ext(file: &[u8], id: u64) -> Option<Vec<u8>> {
    extensions(file)
        .into_iter()
        .find(|e| e.0 == id)
        .map(|e| e.1)
}

/// `file` with extension `id` set to `rec` (None: removed), in id order.
fn with_ext(file: &[u8], id: u64, rec: Option<&[u8]>) -> Vec<u8> {
    let mut recs: Vec<(u64, Vec<u8>)> =
        extensions(file).into_iter().filter(|e| e.0 != id).collect();
    if let Some(r) = rec {
        let at = recs.partition_point(|e| e.0 < id);
        recs.insert(at, (id, r.to_vec()));
    }
    with_extensions(file, &recs)
}

/// `file` with its payload rewritten without the unit table hint (SPEC
/// 2.7): readers find every record by its own length.
fn prefixed(file: &[u8]) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let d = &file[data_payload(file)];
    let p = common::payload(d, &h.info, h.coding == imij::Coding::Lossy);
    with_data(file, &common::without_hint(d, &p))
}

// ---- cases ---------------------------------------------------------------------

struct Base {
    file: Vec<u8>,
    info: ImageInfo,
    reference: Vec<u8>,
    frames: Option<Vec<u8>>,
}

/// A lossless file: its reference is the source.
fn lossless(px: Vec<u8>, info: ImageInfo, file: Vec<u8>) -> Base {
    Base {
        file,
        info,
        reference: px,
        frames: None,
    }
}

/// A lossy file: its reference is `decode`'s output (empty when this build
/// cannot decode it).
fn lossy(info: ImageInfo, file: Vec<u8>) -> Base {
    let reference = imij::decode(&file).map(|d| d.1).unwrap_or_default();
    Base {
        file,
        info,
        reference,
        frames: None,
    }
}

/// A multi-frame file: frame 0 is the reference, every frame the frames'.
fn framed(info: ImageInfo, file: Vec<u8>) -> Base {
    let mut b = lossy(info, file);
    b.frames = all_frames(&b.file);
    b
}

fn all_frames(file: &[u8]) -> Option<Vec<u8>> {
    let mut d = FrameDecoder::new(file).ok()?;
    let animation = d.kind() == imij::FrameKind::Animation;
    let mut canvas = vec![0u8; d.info().checked_buffer_len()?];
    let mut all = Vec::new();
    while d.next_frame().ok()?.is_some() {
        if animation {
            let rb = d.info().row_bytes();
            d.decode_into(&mut canvas, rb, 1).ok()?;
            all.extend_from_slice(&canvas);
        } else if let Some(info) = d.frame_info() {
            let mut page = vec![0u8; info.checked_buffer_len()?];
            d.decode_into(&mut page, info.row_bytes(), 1).ok()?;
            all.extend_from_slice(&page);
        }
    }
    Some(all)
}

struct Cases(Vec<Case>, Vec<(String, imij::Error)>);

impl Cases {
    /// A case this build cannot build (its table says `absent`).
    fn absent(&mut self, name: &str, why: imij::Error) {
        self.1.push((name.into(), why));
    }

    fn add(&mut self, name: &str, base: &Base, file: Vec<u8>) {
        self.add_limited(name, base, file, Limits::DEFAULT);
    }

    fn add_limited(&mut self, name: &str, base: &Base, file: Vec<u8>, limits: Limits) {
        assert!(!self.0.iter().any(|c| c.name == name), "case {name} twice");
        self.0.push(Case {
            name: name.into(),
            file,
            info: base.info.clone(),
            reference: base.reference.clone(),
            frames: base.frames.clone(),
            limits,
        });
    }

    fn base(&mut self, name: &str, base: &Base) {
        self.add(name, base, base.file.clone());
    }
}

fn info(w: u32, h: u32, ch: Channels, s: SampleType) -> ImageInfo {
    ImageInfo::new(w, h, ch, s)
}

/// Every case, in table order, and the cases this build cannot build.
fn cases() -> Cases {
    let mut c = Cases(Vec::new(), Vec::new());
    let meta = metadata();

    // -- lossless files --
    let i = info(40, 30, Channels::Gray, SampleType::U8);
    let px = pixels(&i, 1);
    let one_unit = lossless(
        px.clone(),
        i.clone(),
        enc(
            &px,
            &i,
            &Metadata::default(),
            &imij::ImiEncoderConfig::default(),
        ),
    );
    c.base("ll/gray8-one-unit", &one_unit);

    let i = info(96, 80, Channels::Rgb, SampleType::U8);
    let px = pixels(&i, 2);
    let rgb = lossless(px.clone(), i.clone(), enc(&px, &i, &meta, &lossless_cfg()));
    c.base("ll/rgb8-table", &rgb);
    c.add("ll/rgb8-prefixed", &rgb, prefixed(&rgb.file));
    c.add("ll/rgb8-open-data", &rgb, open_data(&rgb.file));
    c.add(
        "ll/rgb8-open-data-then-chunk",
        &rgb,
        after_data(&open_data(&rgb.file), &chunk(b"vnDs", b"after")),
    );
    c.add(
        "ll/rgb8-chunk-after-data",
        &rgb,
        after_data(&rgb.file, &chunk(b"vnDs", b"after")),
    );
    let mut stale = ext(&rgb.file, 4).expect("a DATA locator");
    stale[2] ^= 1;
    c.add(
        "ll/rgb8-stale-locator",
        &rgb,
        with_ext(&rgb.file, 4, Some(&stale)),
    );
    c.add("ll/rgb8-no-locator", &rgb, with_ext(&rgb.file, 4, None));
    c.add(
        "ll/rgb8-mcmp",
        &rgb,
        imij::compress_metadata(&rgb.file, 1 << 20).unwrap(),
    );

    let i = info(200, 48, Channels::Rgba, SampleType::U16);
    let px = pixels(&i, 3);
    let cfg = {
        let mut cfg = lossless_cfg();
        cfg = cfg.with_column_groups(Some(64));
        cfg
    };
    c.base(
        "ll/rgba16-groups",
        &lossless(
            px.clone(),
            i.clone(),
            enc(&px, &i, &Metadata::default(), &cfg),
        ),
    );

    for (name, ch, s) in [
        ("ll/f16-rgb", Channels::Rgb, SampleType::F16),
        ("ll/i8-gray", Channels::Gray, SampleType::I8),
        ("ll/i16-graya", Channels::GrayAlpha, SampleType::I16),
        ("ll/u32-gray", Channels::Gray, SampleType::U32),
        ("ll/f32-rgb", Channels::Rgb, SampleType::F32),
        ("ll/u64-gray", Channels::Gray, SampleType::U64),
        ("ll/f64-gray", Channels::Gray, SampleType::F64),
    ] {
        let i = info(48, 40, ch, s);
        let px = pixels(&i, 4);
        c.base(
            name,
            &lossless(
                px.clone(),
                i.clone(),
                enc(&px, &i, &Metadata::default(), &lossless_cfg()),
            ),
        );
    }

    let i = info(64, 48, Channels::Rgb, SampleType::U8);
    let px = few_colors(&i);
    // Value-table maps (SPEC 2.1 map 9), and one whose file does not declare
    // value_tables. The encoder writes 8-bit tables only when the lab forces
    // them; other builds code the u8 image with regular maps.
    for (name, ch, s) in [
        ("ll/value-tables-u8", Channels::Gray, SampleType::U8),
        ("ll/value-tables-f16", Channels::Rgb, SampleType::F16),
        ("ll/value-tables-f32", Channels::Rgba, SampleType::F32),
        ("ll/value-tables-u64", Channels::Gray, SampleType::U64),
    ] {
        let i = info(72, 40, ch, s);
        let px = sparse_values(&i);
        let narrow = s.bytes() == 1;
        #[cfg(feature = "lab")]
        if narrow {
            imij::lab::set_value_tables(2);
        }
        let file = enc(&px, &i, &Metadata::default(), &lossless_cfg());
        #[cfg(feature = "lab")]
        imij::lab::set_value_tables(0);
        assert_eq!(
            imij::header(&file)
                .unwrap()
                .features
                .contains(imij::Feature::ValueTables),
            !narrow || cfg!(feature = "lab"),
            "{name} uses value tables"
        );
        let base = lossless(px, i, file);
        c.base(name, &base);
        if s == SampleType::F16 {
            let mask = common::feature_mask(&base.file) & !(1 << imij::Feature::ValueTables.id());
            c.add(
                "ll/value-tables-undeclared",
                &base,
                common::with_feature_mask(&base.file, (mask != 0).then_some(mask)),
            );
        }
    }

    c.base(
        "ll/palette",
        &lossless(
            px.clone(),
            i.clone(),
            enc(&px, &i, &Metadata::default(), &lossless_cfg()),
        ),
    );

    let i = info(64, 48, Channels::Rgb, SampleType::U8);
    let px = pixels(&i, 5);
    let near = enc(
        &px,
        &i,
        &Metadata::default(),
        &lossless_cfg().tune_lossless(|t| t.max_error = [2; 4]),
    );
    // Near-lossless: the reference is the decode (within the bound).
    let near = lossy(i.clone(), near);
    c.base("ll/near2", &near);
    // FloatShift (map 8) in a 16-bit float image: mixed-sign values with
    // seven dead low bits.
    let i = info(128, 32, Channels::Gray, SampleType::F16);
    let px: Vec<u8> = (0u16..4096)
        .flat_map(|x| ((x % 512) * 128).to_ne_bytes())
        .collect();
    let f16_shift = lossless(
        px.clone(),
        i.clone(),
        enc(&px, &i, &Metadata::default(), &{
            let mut cfg = lossless_cfg();
            cfg = cfg.tune_lossless(|t| {
                t.palette = false;
            });
            cfg
        }),
    );
    c.base("ll/f16-floatshift", &f16_shift);
    // Both without the feature that declares near-lossless bounds or the
    // float map: the mask field removed, and the mask kept at 0 (the same
    // length, so no offset moves).
    for (name, b) in [("near2", &near), ("f16-floatshift", &f16_shift)] {
        assert_ne!(
            common::feature_mask(&b.file),
            0,
            "{name} declares its feature"
        );
        c.add(
            &format!("hd/{name}-undeclared"),
            b,
            with_feature_mask(&b.file, None),
        );
        c.add(
            &format!("hd/{name}-hidden"),
            b,
            with_feature_mask(&b.file, Some(0)),
        );
    }

    let i = info(48, 40, Channels::Rgb, SampleType::F32);
    let px = rounded_f32(&i, 6);
    let shifted = enc(&px, &i, &Metadata::default(), &lossless_cfg());
    c.base("ll/f32-floatshift", &lossless(px, i.clone(), shifted));

    // Bilevel stripes (SPEC 2.10): a 1-bit page-like image, with column
    // groups, and its feature and table mutations. Without palettes, so
    // template palette stripes (SPEC 2.14) do not compete.
    let i = info(400, 300, Channels::Gray, SampleType::U8);
    let px = glyph_page(&i);
    let no_palettes = || {
        let mut cfg = lossless_cfg();
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    for (name, groups) in [("ll/bilevel", None), ("ll/bilevel-groups", Some(48))] {
        let cfg = {
            let mut cfg = no_palettes();
            cfg = cfg.with_column_groups(groups);
            cfg
        };
        let b = lossless(
            px.clone(),
            i.clone(),
            enc(&px, &i, &Metadata::default(), &cfg),
        );
        assert_ne!(
            common::feature_mask(&b.file) & BILEVEL,
            0,
            "{name} uses bilevel stripes"
        );
        c.base(name, &b);
    }
    let bl = lossless(
        px.clone(),
        i.clone(),
        enc(&px, &i, &Metadata::default(), &no_palettes()),
    );
    // Two colors in RGB: bilevel palette stripes (0x82, from a near-lossless
    // request, where template palette stripes are not tried; the stripes
    // are exact and the file declares no bounds), and one whose first
    // stripe declares a third entry.
    let rgb2 = info(400, 300, Channels::Rgb, SampleType::U8);
    let px2: Vec<u8> = px
        .iter()
        .flat_map(|&v| {
            if v == 0 {
                [30, 60, 90]
            } else {
                [240, 230, 200]
            }
        })
        .collect();
    let mut near = lossless_cfg();
    near = near.tune_lossless(|t| t.max_error[..3].fill(2));
    let bl2 = lossless(
        px2.clone(),
        rgb2.clone(),
        enc(&px2, &rgb2, &Metadata::default(), &near),
    );
    c.base("ll/bilevel-two-colors", &bl2);
    let d2 = data_payload(&bl2.file);
    let u = common::payload(&bl2.file[d2.clone()], &rgb2, false).records()[0].clone();
    let mut f = bl2.file.clone();
    assert_eq!(f[d2.start + u.start..d2.start + u.start + 2], [0x82, 1]);
    f[d2.start + u.start + 1] = 2;
    c.add("da/bilevel-palette-three-entries", &bl2, f);
    c.add(
        "hd/bilevel-undeclared",
        &bl,
        with_feature_mask(&bl.file, None),
    );
    c.add(
        "hd/bilevel-hidden",
        &bl,
        with_feature_mask(&bl.file, Some(0)),
    );
    let d = data_payload(&bl.file);
    let p = common::payload(&bl.file[d.clone()], &i, false);
    let table = p.head_record(25).expect("the bilevel table");
    // The record's id made even (ignorable): stripes without a table.
    let mut f = bl.file.clone();
    let id_at = d.start + table.start - common::varint_bytes(table.len()).len() - 1;
    assert_eq!(f[id_at], 25);
    f[id_at] = 24;
    c.add("da/bilevel-table-ignored", &bl, f);
    // The former id is unknown in lossless heads, not another spelling.
    let mut f = bl.file.clone();
    f[id_at] = 21;
    c.add("da/bilevel-table-retired-id", &bl, f);
    // Context 0's probability (bits 2..13, after inv and its used bit) set
    // above M.
    let mut f = bl.file.clone();
    let t = d.start + table.start;
    assert_eq!(f[t] >> 1 & 1, 1, "context 0 is used");
    f[t] |= 0xFC;
    f[t + 1] |= 0x1F;
    c.add("da/bilevel-probability-above-1", &bl, f);

    // Template palette stripes (SPEC 2.14): shared entries above, the bottom
    // stripes' own two (stripes of 30 rows, so the row bitmaps have padding),
    // with column groups, and the feature, table and layout mutations.
    let i = info(300, 200, Channels::Gray, SampleType::U8);
    let px = map_page(&i, 150);
    // Regular maps, so the mutations below meet the template syntax first;
    // `ll/template-value-tables` keeps the encoder's value tables.
    let tcfg = |groups, value_tables| {
        let mut cfg = lossless_cfg();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(30);
        });
        cfg = cfg.with_column_groups(groups);
        cfg = cfg.tune_lossless(|t| {
            t.value_tables = value_tables;
        });
        cfg
    };
    // Template stripes over value tables: the same page as 16-bit gray with
    // irregularly spaced values (a 4-bit table where the regular map keeps
    // 16 bits).
    let i16b = info(300, 200, Channels::Gray, SampleType::U16);
    let px16: Vec<u8> = px
        .iter()
        .flat_map(|&v| (v as u16 * 251 + 7).to_ne_bytes())
        .collect();
    let both = enc(&px16, &i16b, &Metadata::default(), &tcfg(None, true));
    assert!(
        imij::header(&both)
            .unwrap()
            .features
            .contains(imij::Feature::ValueTables)
            && common::feature_mask(&both) & TEMPLATE_PALETTES != 0,
        "template stripes over value tables"
    );
    c.base("ll/template-value-tables", &lossless(px16, i16b, both));
    let mut tp = Vec::new();
    for (name, groups) in [("ll/template", None), ("ll/template-groups", Some(48))] {
        let b = lossless(
            px.clone(),
            i.clone(),
            enc(&px, &i, &Metadata::default(), &tcfg(groups, false)),
        );
        assert_ne!(
            common::feature_mask(&b.file) & TEMPLATE_PALETTES,
            0,
            "{name} uses template palette stripes"
        );
        c.base(name, &b);
        tp.push(b);
    }
    let tp = tp.swap_remove(0);
    let mask = common::feature_mask(&tp.file);
    c.add(
        "hd/template-undeclared",
        &tp,
        with_feature_mask(&tp.file, None),
    );
    c.add(
        "hd/template-hidden",
        &tp,
        with_feature_mask(&tp.file, Some(mask & !TEMPLATE_PALETTES)),
    );
    let d = data_payload(&tp.file);
    let p = common::payload(&tp.file[d.clone()], &i, false);
    let units = p.records();
    let kind = |u: &std::ops::Range<usize>| tp.file[d.start + u.start];
    assert!(units.iter().any(|u| kind(u) == 0x84) && units.iter().any(|u| kind(u) == 0x83));
    let table = p.head_record(35).expect("the template tables");
    let id_at = d.start + table.start - common::varint_bytes(table.len()).len() - 1;
    assert_eq!(tp.file[id_at], 35);
    // The record's id made even (ignorable): stripes without their tables.
    let mut f = tp.file.clone();
    f[id_at] = 34;
    c.add("da/template-table-ignored", &tp, f);
    // An unassigned odd id (39 now declares local ANS models; the
    // registry in imij/src/registry.rs lists every assigned id).
    let mut f = tp.file.clone();
    f[id_at] = 41;
    c.add("da/template-table-unknown-id", &tp, f);
    // The precision field (bits 0..3) changed: the table no longer fills
    // its record.
    let mut f = tp.file.clone();
    f[d.start + table.start] ^= 1;
    c.add("da/template-precision-changed", &tp, f);
    // A shared-entry stripe relabeled with its own entries, and an
    // unassigned layout byte.
    let shared = units.iter().find(|u| kind(u) == 0x84).unwrap();
    let mut f = tp.file.clone();
    f[d.start + shared.start] = 0x83;
    c.add("da/template-shared-relabeled-own", &tp, f);
    let mut f = tp.file.clone();
    f[d.start + shared.start] = 0x85;
    c.add("da/template-unknown-layout", &tp, f);
    // A padding bit of a 30-row stripe's row bitmap set.
    let mut f = tp.file.clone();
    f[d.start + shared.start + 4] |= 0x80;
    c.add("da/template-row-bitmap-padding", &tp, f);

    // Checksums.
    let csum = lossless(
        rgb.reference.clone(),
        rgb.info.clone(),
        enc(
            &rgb.reference,
            &rgb.info,
            &meta,
            &lossless_cfg().with_checksum(true),
        ),
    );
    c.base("ll/csum", &csum);
    let base = common::checksum::strip(&csum.file);
    let k = csum.file[common::checksum::record_at(&csum.file)];
    use common::checksum::{Place, with_table};
    c.add("ll/csum-last", &csum, with_table(&base, k, 8, Place::Last));
    c.add(
        "ll/csum-k10-w4-first",
        &csum,
        with_table(&base, 10, 4, Place::Chunk(0)),
    );

    // Previews and a gain map.
    let pi = info(24, 20, Channels::Rgb, SampleType::U8);
    let ppx = pixels(&pi, 7);
    let previews = lossless_cfg()
        .encode_samples_with_previews(
            imij::ImageRef::new(&rgb.reference, rgb.info.row_bytes(), &rgb.info),
            &Metadata::default(),
            &[imij::ImageRef {
                pixels: &ppx,
                stride: pi.row_bytes(),
                info: &pi,
            }],
        )
        .unwrap();
    c.add("ll/previews", &rgb, previews);
    let gi = info(24, 20, Channels::Gray, SampleType::U8);
    let gain = Metadata {
        gain_map: Some(imij::GainMap {
            params: imij::GainMapParams::default(),
            alt_cicp: Some(Cicp::BT2100_PQ),
            alt_icc: None,
            image: enc(
                &pixels(&gi, 8),
                &gi,
                &Metadata::default(),
                &imij::ImiEncoderConfig::default(),
            ),
        }),
        ..Default::default()
    };
    c.add(
        "ll/gainmap",
        &rgb,
        enc(&rgb.reference, &rgb.info, &gain, &lossless_cfg()),
    );

    // -- lossy files --
    let i = info(32, 24, Channels::Rgb, SampleType::U8);
    let ly1 = lossy(
        i.clone(),
        enc_lossy(
            &pixels(&i, 9),
            &i,
            &Metadata::default(),
            &imij::ImiEncoderConfig::lossy(70.0),
        ),
    );
    c.base("ly/xyb-one-unit", &ly1);
    let i = info(96, 80, Channels::Rgb, SampleType::U8);
    let ly = lossy(
        i.clone(),
        enc_lossy(&pixels(&i, 10), &i, &meta, &lossy_cfg()),
    );
    c.base("ly/xyb-table", &ly);
    c.add("ly/xyb-prefixed", &ly, prefixed(&ly.file));
    c.add("ly/xyb-open-data", &ly, open_data(&ly.file));
    let i = info(80, 48, Channels::Rgba, SampleType::U8);
    let lya = lossy(
        i.clone(),
        enc_lossy(
            &pixels(&i, 11),
            &i,
            &Metadata::default(),
            &lossy_cfg().tune_lossy(|t| t.color = imij::LossyColor::YCoCg),
        ),
    );
    c.base("ly/ycocg-rgba", &lya);
    c.add("ly/ycocg-rgba-prefixed", &lya, prefixed(&lya.file));
    for (name, speed) in [
        ("ly/fast", DecodeSpeed::Fast),
        ("ly/fastest", DecodeSpeed::Fastest),
        ("ly/compact", DecodeSpeed::Compact),
    ] {
        let i = info(96, 64, Channels::Rgb, SampleType::U8);
        c.base(
            name,
            &lossy(
                i.clone(),
                enc_lossy(
                    &pixels(&i, 12),
                    &i,
                    &Metadata::default(),
                    &lossy_cfg().with_decode_speed(speed),
                ),
            ),
        );
    }
    let i = info(96, 64, Channels::Rgb, SampleType::U8);
    c.base(
        "ly/splithuf",
        &lossy(
            i.clone(),
            enc_lossy(
                &pixels(&i, 13),
                &i,
                &Metadata::default(),
                &lossy_cfg().tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
            ),
        ),
    );
    let mut g = lossy_cfg();
    g = g.with_column_groups(Some(32));
    c.base(
        "ly/groups",
        &lossy(
            i.clone(),
            enc_lossy(&pixels(&i, 14), &i, &Metadata::default(), &g),
        ),
    );
    let mut e = lossy_cfg();
    e = e.tune_lossy(|t| {
        t.exact_blocks = Some(1000.0);
    });
    let exact = lossy(
        i.clone(),
        enc_lossy(&few_colors(&i), &i, &Metadata::default(), &e),
    );
    c.base("ly/exact", &exact);
    let exact_csum = lossy(
        i.clone(),
        enc_lossy(
            &few_colors(&i),
            &i,
            &Metadata::default(),
            &e.clone().with_checksum(true),
        ),
    );
    c.base("ly/exact-csum", &exact_csum);
    c.base(
        "ly/csum",
        &lossy(
            i.clone(),
            enc_lossy(&pixels(&i, 15), &i, &meta, &lossy_cfg().with_checksum(true)),
        ),
    );
    // Superblocks (SPEC 2.9): a Compact file that merges, the same file
    // with tree value 7 in its first merging unit's map, and with record 27
    // (the default layouts' bytes) beside record 21.
    let i = info(160, 128, Channels::Rgb, SampleType::U8);
    let sb = lossy(
        i.clone(),
        enc_lossy(
            &pixels(&i, 19),
            &i,
            &Metadata::default(),
            &imij::ImiEncoderConfig::lossy(30.0).with_decode_speed(DecodeSpeed::Compact),
        ),
    );
    let lens = common::superblock_map_lengths(&sb.file, &i);
    let u = lens
        .iter()
        .position(|&n| n > 0)
        .expect("a unit that merges");
    c.base("ly/superblocks", &sb);
    c.add(
        "ly/superblocks-map-symbol",
        &sb,
        common::with_superblock_map(&sb.file, u, &common::superblock_map(2, 2, |_, _| 8)),
    );
    // (Declared, so the superblocks rule is what fails.)
    c.add(
        "ly/superblocks-with-layouts",
        &sb,
        with_feature_mask(
            &common::with_lossy_head_record(&sb.file, 27, Some(&[8, 8, 8])),
            Some(common::feature_mask(&sb.file) | CHANNEL_LAYOUTS),
        ),
    );
    // Small transforms (SPEC 2.11): a forged joint map on a 16x16 file of
    // channel noise (one unit, four parents, every plane with AC: lab
    // builds check it), and malformed variants.
    let i = info(16, 16, Channels::Rgb, SampleType::U8);
    let noise: Vec<u8> = (0..16 * 16 * 3u32)
        .map(|k| (k.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let plain = enc_lossy(
        &noise,
        &i,
        &Metadata::default(),
        &imij::ImiEncoderConfig::lossy(90.0),
    );
    #[cfg(feature = "lab")]
    assert_eq!(
        imij::lab::lossy_unit_ac_bits(&plain).unwrap(),
        [vec![7u8; 4]]
    );
    let small_file = |map: &[u8], sb: bool| {
        let f = common::with_lossy_head_record(&plain, common::H_PARTITIONS, Some(&[]));
        let mask = common::feature_mask(&f)
            | common::feature::SMALL_TRANSFORMS
            | if sb { common::feature::SUPERBLOCKS } else { 0 };
        common::with_inserted_map(&with_feature_mask(&f, Some(mask)), 0, map)
    };
    let all = |_: usize, _: usize| 7u8;
    let split = common::joint_map(
        2,
        2,
        |_, _| 0,
        |r, x| Some(((r * 2 + x) as u8 + 1) & 7),
        all,
    );
    let small = lossy(i.clone(), small_file(&split, false));
    c.base("ly/small", &small);
    let mut grown = split.clone();
    grown.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0x5a]);
    c.add("ly/small-map-unread", &small, small_file(&grown, false));
    c.add(
        "ly/small-undeclared",
        &small,
        with_feature_mask(
            &small.file,
            Some(common::feature_mask(&small.file) & !common::feature::SMALL_TRANSFORMS),
        ),
    );
    c.add(
        "ly/small-with-superblocks",
        &small,
        with_feature_mask(
            &common::with_lossy_head_record(&small.file, common::H_SUPERBLOCKS, Some(&[])),
            Some(common::feature_mask(&small.file) | common::feature::SUPERBLOCKS),
        ),
    );
    let merge = common::joint_map(2, 2, |_, _| 1, |_, _| None, all);
    c.add(
        "ly/small-merge-undeclared",
        &small,
        small_file(&merge, false),
    );
    c.base(
        "ly/small-merge",
        &lossy(i.clone(), small_file(&merge, true)),
    );
    let empty = common::joint_map_with_header(2, 2, |_, _| 1, |_, _| None, all, Some((true, true)));
    c.add("ly/small-empty-section", &small, small_file(&empty, true));
    for (name, s) in [
        ("ly/u16", SampleType::U16),
        ("ly/f32", SampleType::F32),
        ("ly/u32", SampleType::U32),
    ] {
        let mut i = info(48, 40, Channels::Rgb, s);
        if s == SampleType::U16 {
            i.cicp = Cicp::BT2100_PQ;
        }
        let cfg = lossy_cfg().tune_lossy(|t| t.color = imij::LossyColor::YCoCg);
        let px = pixels(&i, 16);
        // Builds without `extended` do not encode lossy U32. F32 and its
        // internal 32-bit coefficients are available in every build.
        match cfg.encode_legacy_samples(
            imij::ImageRef::new(&px, i.row_bytes(), &i),
            &Metadata::default(),
        ) {
            Ok(f) => c.base(name, &lossy(i.clone(), f)),
            Err(e) => c.absent(name, e),
        }
    }

    // -- frames and pages --
    let i = info(48, 32, Channels::Rgba, SampleType::U8);
    let mut a = FrameEncoder::animation(
        i.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    let mut px = pixels(&i, 17);
    for f in 0..3 {
        for v in px.iter_mut().skip(f * 97).step_by(13) {
            *v = v.wrapping_add(31);
        }
        a.push_frame(&px, i.row_bytes(), FrameDuration::from_millis(40))
            .unwrap();
    }
    let anim = framed(i.clone(), a.finish_legacy().unwrap());
    c.base("fr/animation", &anim);
    let mut p = FrameEncoder::pages(
        i.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::lossy(60.0)),
    );
    for (k, (w, h)) in [(48, 32), (40, 24)].into_iter().enumerate() {
        let pi = info(w, h, Channels::Rgba, SampleType::U8);
        p.push_page(&pixels(&pi, 18 + k as u32), pi.row_bytes(), &pi)
            .unwrap();
    }
    c.base("fr/pages", &framed(i.clone(), p.finish_legacy().unwrap()));
    let mixed = |checksum: bool| {
        let mut m = FrameEncoder::mixed_pages(FrameConfig::new(
            imij::ImiEncoderConfig::default().with_checksum(checksum),
        ));
        let first = info(40, 24, Channels::Gray, SampleType::U8);
        let pages = [
            (first.clone(), imij::ImiEncoderConfig::default()),
            (
                info(32, 24, Channels::Rgb, SampleType::U8),
                imij::ImiEncoderConfig::lossy(60.0),
            ),
            (
                info(19, 23, Channels::Rgba, SampleType::U16),
                imij::ImiEncoderConfig::default(),
            ),
        ];
        for (k, (pi, coding)) in pages.iter().enumerate() {
            m.push_page_with(
                &pixels(pi, 21 + k as u32),
                pi.row_bytes(),
                pi,
                &Metadata::default(),
                coding,
            )
            .unwrap();
        }
        framed(first, m.finish_legacy().unwrap())
    };
    let mixed_plain = mixed(false);
    c.base("fr/mixed", &mixed_plain);
    c.base("fr/mixed-csum", &mixed(true));
    // Near-lossless animation frames, then with every header's
    // `near_lossless` bit (the outer file's and each frame's) cleared, the
    // one-byte masks kept so no offset moves.
    let i = info(64, 16, Channels::Gray, SampleType::U8);
    let near_cfg = {
        let mut cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [1; 4]);
        cfg = cfg.tune_lossless(|t| {
            t.palette = false;
        });
        cfg
    };
    let mut a = FrameEncoder::animation(i.clone(), Metadata::default(), FrameConfig::new(near_cfg));
    let mut px = pixels(&i, 24);
    for _ in 0..2 {
        a.push_frame(&px, i.row_bytes(), FrameDuration::from_millis(40))
            .unwrap();
        px.reverse();
    }
    let near_anim = framed(i.clone(), a.finish_legacy().unwrap());
    c.base("fr/near-animation", &near_anim);
    let mut hidden = near_anim.file.clone();
    let mut n = 0;
    let mut at = 0;
    while let Some(p) = hidden[at..].windows(8).position(|w| w == common::SIGNATURE) {
        let sig = at + p;
        let h = common::header_record(&hidden[sig..]);
        let l = common::header_layout(&hidden[sig + h.start..sig + h.end]);
        let m = sig + h.start + l.features.start;
        if l.features.len() == 1 && hidden[m] as u64 & NEAR_LOSSLESS != 0 {
            hidden[m] &= !(NEAR_LOSSLESS as u8);
            n += 1;
        }
        at = sig + 8;
    }
    assert!(n >= 2, "outer and frame masks: {n}");
    c.add("fr/near-animation-hidden", &near_anim, hidden);
    // `fRAM` controls (SPEC 1.3): an unknown presence bit, then a frame
    // record list holding an ignorable record or an unknown critical one.
    let present = |doc: &[u8]| {
        let (a, _) = find(doc, b"fRAM").unwrap();
        let k = common::varint(&doc[a + 4..]).1;
        let mut at = a + 4 + k;
        for _ in 0..4 {
            common::read_varint(doc, &mut at);
        }
        doc[at]
    };
    let p0 = present(&anim.file);
    assert!(p0 < 4);
    c.add(
        "fr/frame-unknown-field",
        &anim,
        with_frame_controls(&anim.file, &[p0 | 8]),
    );
    c.add(
        "fr/frame-ignorable-record",
        &anim,
        with_frame_controls(&anim.file, &[p0 | 4, 3, 2, 1, 0xaa]),
    );
    c.add(
        "fr/frame-unknown-record",
        &anim,
        with_frame_controls(&anim.file, &[p0 | 4, 2, 1, 0]),
    );

    // -- container and header mutations (of ll/rgb8-table) --
    let f = &rgb.file;
    let mut sig = f.clone();
    sig[0] ^= 0x20;
    c.add("hd/signature", &rgb, sig);
    c.add("hd/header-version", &rgb, with_header_byte(f, 0, 5));
    c.add("hd/header-version-1", &rgb, with_header_byte(f, 0, 1));
    // A version 2 header over version 3 unit records: the ids do not translate.
    c.add("hd/header-version-2", &rgb, with_header_byte(f, 0, 2));
    c.add("hd/unknown-critical-ext", &rgb, with_ext(f, 5, Some(&[1])));
    c.add(
        "hd/unknown-ignorable-ext",
        &rgb,
        with_ext(f, 6, Some(&[1, 2])),
    );
    // The locator (4) and an ignorable record (6), in the wrong order.
    let mut recs = extensions(f);
    recs.push((6, vec![1]));
    assert!(recs.len() >= 2);
    recs.reverse();
    c.add("hd/ext-out-of-order", &rgb, with_extensions(f, &recs));
    c.add(
        "hd/feature-unknown-bit",
        &rgb,
        with_feature_mask(f, Some(common::unassigned_feature())),
    );
    c.add(
        "hd/retired-profiles-record",
        &rgb,
        with_ext(f, 1, Some(&[0, 0, 1])),
    );
    c.add(
        "hd/mixed-signal-no-anim",
        &rgb,
        with_feature_mask(f, Some(MIXED_PAGES)),
    );
    c.add(
        "hd/huge-dims",
        &rgb,
        common::with_dimensions(f, 100_000, 100_000),
    );
    let tall = common::with_dimensions(f, 96, 81);
    c.add("hd/one-row-more", &rgb, tall);
    // Varints and lengths, on a 1x2 gray image whose header record is
    // version 2, present = channels, Gray, width 1, height 2.
    let tiny_info = info(1, 2, Channels::Gray, SampleType::U8);
    let tiny_px = vec![0u8, 1];
    let tiny_file = enc(
        &tiny_px,
        &tiny_info,
        &Metadata::default(),
        &imij::ImiEncoderConfig::default(),
    );
    let tiny = lossless(tiny_px, tiny_info, tiny_file);
    let (th, tc) = common::parts(&tiny.file);
    assert_eq!(th, [3, 8, 1, 1, 2]);
    let with_width = |w: &[u8]| common::join(&[&th[..3], w, &th[4..]].concat(), &tc, false);
    // An overflowing width never aliases a small one.
    for last in [2u8, 3, 127, 128, 255] {
        let w = [&[0x81][..], &[0x80; 8], &[last]].concat();
        c.add(&format!("hd/width-overflow-{last}"), &tiny, with_width(&w));
    }
    // Non-minimal encodings of a valid width: one value, one encoding.
    for (n, w) in [
        &[0x81, 0x00][..],
        &[0x81, 0x80, 0x00],
        &[0x81, 0x80, 0x80, 0x00],
    ]
    .into_iter()
    .enumerate()
    {
        c.add(&format!("hd/width-padded-{}", n + 2), &tiny, with_width(w));
    }
    // Lengths of 2^32 and more do not alias small ones on 32-bit hosts.
    let mut long = common::SIGNATURE.to_vec();
    put_varint(&mut long, (1u64 << 32) + th.len() as u64);
    long.extend(&th);
    for (tag, payload) in &tc {
        common::put_chunk(&mut long, tag, payload);
    }
    c.add("hd/header-len-over-4g", &tiny, long);
    let mut long = common::join(&th, &[], false);
    for (tag, payload) in &tc {
        long.extend(tag);
        put_varint(&mut long, (1u64 << 32) + payload.len() as u64 + 1);
        long.extend(payload);
    }
    c.add("ch/len-over-4g", &tiny, long);
    // The same image with CICP and alpha mode written explicitly (present =
    // channels | CICP | alpha mode, Gray, CICP 1/13/0/1, straight alpha,
    // 1x2), then with a reserved alpha mode and a full-range flag of 2.
    let explicit = [3, 0xc8, 0x02, 1, 1, 13, 0, 1, 0, 1, 2];
    c.add(
        "hd/explicit-cicp-alpha",
        &tiny,
        common::join(&explicit, &tc, false),
    );
    for (name, at, v) in [
        ("hd/reserved-alpha-mode", 8, 0x80),
        ("hd/full-range-2", 7, 2),
    ] {
        let mut h = explicit.to_vec();
        h[at] = v;
        c.add(name, &tiny, common::join(&h, &tc, false));
    }
    let mut bad_tag = tc.clone();
    bad_tag.insert(0, (*b"a!\0?", Vec::new()));
    c.add(
        "ch/bad-tag-bytes",
        &tiny,
        common::join(&th, &bad_tag, false),
    );

    // -- chunk mutations --
    c.add(
        "ch/dup-exif",
        &rgb,
        before_data(f, &chunk(b"eXIf", b"second")),
    );
    c.add(
        "ch/dup-iccp-after-data",
        &rgb,
        after_data(f, &chunk(b"iCCP", b"second")),
    );
    c.add(
        "ch/bad-mdcv",
        &rgb,
        before_data(f, &chunk(b"mDCv", &[0; 23])),
    );
    c.add(
        "ch/bad-clli",
        &rgb,
        before_data(f, &chunk(b"cLLI", &[0; 5])),
    );
    c.add(
        "ch/good-mdcv-clli",
        &rgb,
        before_data(
            &before_data(f, &chunk(b"mDCv", &[0; 24])),
            &chunk(b"cLLI", &[0; 4]),
        ),
    );
    let packed = imij::compress_metadata(f, 1 << 20).unwrap();
    let (ma, mb) = find(&packed, b"mCMP").expect("an mCMP chunk");
    let mp = ma + 4 + varint(&packed[ma + 4..]).1;
    let mut bad = packed.clone();
    bad[mp..mp + 4].copy_from_slice(b"vnDr");
    c.add("ch/mcmp-bad-identity", &rgb, bad);
    let mut bad = packed.clone();
    for b in &mut bad[mp + 4..mb] {
        *b = 0xff;
    }
    c.add("ch/mcmp-bad-pack", &rgb, bad);
    c.add(
        "ch/anim-after-data",
        &rgb,
        after_data(f, &chunk(b"aNIM", &[1, 0, 1, 1, 1])),
    );
    c.add(
        "ch/anim-bad-kind",
        &rgb,
        before_data(f, &chunk(b"aNIM", &[9, 0, 1, 1, 1])),
    );
    c.add(
        "ch/anim-count-mismatch",
        &rgb,
        before_data(f, &chunk(b"aNIM", &[1, 0, 3, 1, 1])),
    );
    c.add(
        "ch/fram-without-anim",
        &rgb,
        after_data(f, &chunk(b"fRAM", &[0, 0, 1, 1, 0])),
    );
    c.add(
        "ch/unknown-critical-before",
        &rgb,
        before_data(f, &chunk(b"ZZZZ", b"x")),
    );
    c.add(
        "ch/unknown-critical-after",
        &rgb,
        after_data(f, &chunk(b"ZZZZ", b"x")),
    );
    let mut open = b"vnDs".to_vec();
    open.push(0);
    c.add("ch/open-ancillary", &rgb, before_data(f, &open));
    let mut open = b"ZZZZ".to_vec();
    open.push(0);
    c.add("ch/open-unknown-critical", &rgb, before_data(f, &open));
    c.add("ch/bad-tag", &rgb, before_data(f, &chunk(b"v1Dr", b"x")));
    c.add("ch/no-data", &rgb, without(f, b"DATA"));
    let (da, db) = find(f, b"DATA").unwrap();
    c.add("ch/dup-data", &rgb, after_data(f, &f[da..db]));
    c.add("ch/truncate-1", &rgb, f[..f.len() - 1].to_vec());
    c.add(
        "ch/truncate-header",
        &rgb,
        f[..header_record(f).end].to_vec(),
    );
    c.add("ch/truncate-mid-data", &rgb, f[..(da + db) / 2].to_vec());
    c.add("ch/trailing-garbage", &rgb, [&f[..], b"abc"].concat());
    c.add(
        "ch/chunk-past-end",
        &rgb,
        [&f[..], &b"vnDs\x7f"[..]].concat(),
    );

    // -- DATA mutations --
    let lyf = &ly.file;
    let ll_payload = &f[data_payload(f)];
    let ly_payload = &lyf[data_payload(lyf)];
    c.add(
        "da/lossless-header-lossy-data",
        &rgb,
        with_data(f, ly_payload),
    );
    c.add(
        "da/lossy-header-lossless-data",
        &ly,
        with_data(lyf, ll_payload),
    );
    c.add(
        "da/trailing-byte",
        &rgb,
        with_data(f, &[ll_payload, &[0]].concat()),
    );
    // A byte after the last record, in each lossy entropy mode.
    let ei = info(9, 9, Channels::Rgb, SampleType::U8);
    let mut split_huf = None;
    for (name, entropy) in [
        ("runlevel", LossyEntropy::RunLevel),
        ("splithuf", LossyEntropy::SplitHuf),
    ] {
        let cfg = imij::ImiEncoderConfig::lossy(85.0).tune_lossy(|t| t.entropy = entropy);
        let e = lossy(
            ei.clone(),
            enc_lossy(&[71; 9 * 9 * 3], &ei, &Metadata::default(), &cfg),
        );
        let ef = &e.file;
        let trailing = [&ef[data_payload(ef)], &[0x55]].concat();
        c.add(
            &format!("da/{name}-trailing-byte"),
            &e,
            with_data(ef, &trailing),
        );
        if entropy == LossyEntropy::SplitHuf {
            split_huf = Some(e);
        }
    }
    // Retired entropy values 1 (packed) and 2 (split) are reserved:
    // Unsupported on every decode route (SPEC 2.8).
    {
        let e = split_huf.unwrap();
        let ef = &e.file;
        let d = data_payload(ef).start;
        let p = common::payload(&ef[d..], &ei, true);
        let entropy_at = d
            + p.present_at
            + common::varint(&ef[d + p.present_at..]).1
            + 4
            + usize::from(p.present & 1 != 0);
        assert_eq!(ef[entropy_at], LossyEntropy::SplitHuf as u8);
        for v in [1, 2] {
            let mut bad = ef.clone();
            bad[entropy_at] = v;
            c.add(
                &format!("da/entropy-{v}"),
                &lossy(ei.clone(), bad.clone()),
                bad,
            );
        }
    }
    c.add(
        "da/short-by-one",
        &rgb,
        with_data(f, &ll_payload[..ll_payload.len() - 1]),
    );
    c.add(
        "da/lossy-trailing-byte",
        &ly,
        with_data(lyf, &[ly_payload, &[0]].concat()),
    );
    c.add(
        "da/lossy-short-by-one",
        &ly,
        with_data(lyf, &ly_payload[..ly_payload.len() - 1]),
    );
    let pf = prefixed(f);
    let pp = &pf[data_payload(&pf)];
    c.add(
        "da/prefixed-trailing-byte",
        &rgb,
        with_data(&pf, &[pp, &[0]].concat()),
    );
    c.add(
        "da/prefixed-short-by-one",
        &rgb,
        with_data(&pf, &pp[..pp.len() - 1]),
    );
    let lpf = prefixed(lyf);
    let lpp = &lpf[data_payload(&lpf)];
    c.add(
        "da/lossy-prefixed-trailing-byte",
        &ly,
        with_data(&lpf, &[lpp, &[0]].concat()),
    );
    c.add(
        "da/lossy-prefixed-short-by-one",
        &ly,
        with_data(&lpf, &lpp[..lpp.len() - 1]),
    );
    // Payloads of 0 and 1 byte (the version byte alone), with a length and
    // open at the end of the file: the payload head's first peek reaches
    // the payload's end on every route.
    for (name, e, file, payload) in [
        ("lossless", &rgb, f, ll_payload),
        ("lossy", &ly, lyf, ly_payload),
    ] {
        for n in [0, 1] {
            c.add(
                &format!("da/{name}-{n}-byte-payload"),
                e,
                with_data(file, &payload[..n]),
            );
            c.add(
                &format!("da/{name}-{n}-byte-open-payload"),
                e,
                open_data_at_end(file, &payload[..n]),
            );
        }
    }
    // A head presence bit this version does not define (lossless bit 5;
    // lossy bit 10).
    let mut bad = ll_payload.to_vec();
    bad[0] |= 32;
    c.add("da/lossless-unknown-field", &rgb, with_data(f, &bad));
    let lp = common::payload(ly_payload, &ly.info, true);
    let mut bad = ly_payload[..lp.present_at].to_vec();
    put_varint(&mut bad, lp.present | 1 << 10);
    bad.extend_from_slice(
        &ly_payload[lp.present_at + common::varint(&ly_payload[lp.present_at..]).1..],
    );
    c.add("da/lossy-unknown-flag", &ly, with_data(lyf, &bad));
    // A unit table hint far longer than the records the geometry names (64
    // KiB for one lossless unit, and for the lossy file's few records): its
    // length bounds its entries (1 to 10 bytes each), so every route refuses
    // it before decoding it (review 2026-10-10 #2).
    let long = vec![2u8; 1 << 16];
    let of = &one_unit.file;
    let op = &of[data_payload(of)];
    let hp = common::payload(op, &one_unit.info, false);
    c.add(
        "da/lossless-hint-too-long",
        &one_unit,
        with_data(
            of,
            &common::with_head_record(op, &hp, common::H_UNIT_TABLE, Some(&long)),
        ),
    );
    let lp = common::payload(ly_payload, &ly.info, true);
    c.add(
        "da/lossy-hint-too-long",
        &ly,
        with_data(
            lyf,
            &common::with_head_record(ly_payload, &lp, common::H_UNIT_TABLE, Some(&long)),
        ),
    );

    unit_table_cases(&mut c, [&one_unit, &rgb, &ly1, &ly, &lya]);
    unit_table_checksum_cases(&mut c, &csum);

    // Run tokens (SPEC 3.1, 3.2) from the independent writer: a model that
    // weights a run longer than the cap (16 extra bits, which the narrow
    // SIMD tables once held as 15), and a run past its lane's last
    // position.
    {
        let len = (1 << 16) + 1;
        let i = info(len, 1, Channels::Gray, SampleType::U8);
        let f = common::one_lane_k4(len, 1, &[common::run_token(len)]);
        let cap = lossless(vec![0; len as usize], i, f.clone());
        c.add("da/run-past-cap", &cap, f);
        let i = info(8, 4, Channels::Gray, SampleType::U8);
        let f = common::one_lane_k4(8, 4, &[common::value_token(5), common::run_token(40)]);
        let lane = lossless(vec![253; 32], i, f.clone());
        c.add("da/run-past-lane", &lane, f);
    }

    // -- checksum mutations (of ll/csum) --
    let cf = &csum.file;
    let (ca, cb) = common::checksum::csum(cf);
    let mut bad = cf.clone();
    bad[cb - 1] ^= 1;
    c.add("cs/digest-flip", &csum, bad);
    let (va, vb) = find(cf, b"vnDr").unwrap();
    let mut bad = cf.clone();
    bad[(va + vb) / 2] ^= 1;
    c.add("cs/vendor-byte-flip", &csum, bad);
    let mut bad = cf.clone();
    let p = data_payload(cf);
    bad[p.start + p.len() / 2] ^= 1;
    c.add("cs/data-byte-flip", &csum, bad);
    c.add(
        "cs/record-without-table",
        &csum,
        common::checksum::strip(cf),
    );
    c.add(
        "cs/table-without-record",
        &csum,
        common::checksum::drop_record(cf),
    );
    c.add("cs/table-k-differs", &csum, {
        let t = with_table(&base, 10, 8, Place::BeforeData);
        let mut f2 = t.clone();
        f2[common::checksum::record_at(&t)] = 11;
        f2
    });
    c.add("cs/bad-record-k", &csum, {
        let mut f2 = cf.clone();
        f2[common::checksum::record_at(cf)] = 5;
        f2
    });
    c.add("cs/bad-record-k-no-table", &csum, {
        let mut f2 = common::checksum::strip(cf);
        let at = common::checksum::record_at(&f2);
        f2[at] = 5;
        f2
    });
    c.add("cs/second-table", &csum, after_data(cf, &cf[ca..cb]));
    let unknown = {
        let mut t = cf[ca..cb].to_vec();
        let p = 4 + varint(&t[4..]).1;
        t[p] = 7;
        t
    };
    // The header record names the same unknown algorithm (SPEC 1.4.2).
    let unknown_file = {
        let mut f2 = [&cf[..ca], &unknown, &cf[cb..]].concat();
        let rec = common::checksum::record_at(&f2) + 1;
        f2[rec] = 7;
        f2
    };
    c.add("cs/unknown-algo", &csum, unknown_file.clone());
    c.add(
        "cs/unknown-algo-then-second",
        &csum,
        after_data(&unknown_file, &cf[ca..cb]),
    );
    // The table's algorithm differs from the record's: corrupt either way.
    c.add(
        "cs/record-algo-differs",
        &csum,
        [&cf[..ca], &unknown, &cf[cb..]].concat(),
    );
    c.add("cs/unknown-record-algo", &csum, {
        let mut f2 = cf.to_vec();
        let rec = common::checksum::record_at(&f2) + 1;
        f2[rec] = 7;
        f2
    });
    // The last exact record that holds bytes, of a lossy file with exact
    // blocks and a checksum (B3; an exact record is empty when no nested
    // stripe starts in its unit).
    let ef = &exact_csum.file;
    let ep = data_payload(ef);
    let units = common::payload(&ef[ep.clone()], &exact_csum.info, true).units;
    let last = units
        .iter()
        .rev()
        .find(|u| u.0 == 57 && u.2.end > u.2.start)
        .map(|u| u.2.clone())
        .unwrap();
    let mut bad = ef.clone();
    bad[ep.start + (last.start + last.end) / 2] ^= 1;
    c.add("cs/exact-record-flip", &exact_csum, bad);
    // A mixed page carrying its own checksum table: good, then with a
    // changed digest (B4).
    let nested_info = info(32, 24, Channels::Rgb, SampleType::U8);
    let nested = enc(
        &pixels(&nested_info, 30),
        &nested_info,
        &Metadata::default(),
        &imij::ImiEncoderConfig::default().with_checksum(true),
    );
    let (_, nb) = common::checksum::csum(&nested);
    let mut bad_nested = nested.clone();
    bad_nested[nb - 1] ^= 1;
    let page = |n: &[u8]| {
        let (fa, fb) = find(&mixed_plain.file, b"fRAM").unwrap();
        let mut payload = vec![0, 0, 0, 1, 0];
        payload.extend_from_slice(n);
        [
            &mixed_plain.file[..fa],
            &chunk(b"fRAM", &payload),
            &mixed_plain.file[fb..],
        ]
        .concat()
    };
    let own = framed(mixed_plain.info.clone(), page(&nested));
    c.base("cs/mixed-page-own-table", &own);
    c.add("cs/mixed-page-own-table-bad", &own, page(&bad_nested));

    // Uniform pages and animation frames whose nested file carries its own
    // table, with a changed digest (B4, mainfix W2).
    let replace_frame = |doc: &[u8], nested: &[u8], flags: u8| {
        let (fa, fb) = find(doc, b"fRAM").unwrap();
        let mut payload = vec![0, 0, 0, 1, flags];
        payload.extend_from_slice(nested);
        [&doc[..fa], &chunk(b"fRAM", &payload), &doc[fb..]].concat()
    };
    let pi = info(48, 32, Channels::Rgba, SampleType::U8);
    let page_cfg = imij::ImiEncoderConfig::default().with_checksum(true);
    let own = enc(&pixels(&pi, 31), &pi, &Metadata::default(), &page_cfg);
    let (_, ob) = common::checksum::csum(&own);
    let mut bad_own = own.clone();
    bad_own[ob - 1] ^= 1;
    let mut up = FrameEncoder::pages(
        pi.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    for k in 0..2 {
        up.push_page(&pixels(&pi, 32 + k), pi.row_bytes(), &pi)
            .unwrap();
    }
    let uniform = up.finish_legacy().unwrap();
    let own_page = framed(pi.clone(), replace_frame(&uniform, &own, 0));
    c.base("cs/page-own-table", &own_page);
    c.add(
        "cs/page-own-table-bad",
        &own_page,
        replace_frame(&uniform, &bad_own, 0),
    );
    let own_frame = framed(anim.info.clone(), replace_frame(&anim.file, &own, 1));
    c.base("cs/frame-own-table", &own_frame);
    c.add(
        "cs/frame-own-table-bad",
        &own_frame,
        replace_frame(&anim.file, &bad_own, 1),
    );
    // A file whose checksum table follows DATA: readers deliver rows before
    // the table arrives, so a changed DATA byte shows after them (SPEC 1.4).
    let last = with_table(&base, k, 8, Place::Last);
    let lp = data_payload(&last);
    let mut bad = last.clone();
    bad[lp.start + lp.len() / 2] ^= 1;
    c.add("cs/table-last-data-flip", &csum, bad);
    // The table first, in one piece larger than a reader reads ahead (2^23
    // bytes): the reader hashes it as it serves it, so rows come from
    // unchecked bytes, and the mismatch explains the decode error (it reads
    // on to the piece's end).
    let large = with_table(&base, 23, 8, Place::BeforeData);
    let lp = data_payload(&large);
    let mut bad = large.clone();
    bad[lp.start + lp.len() / 2] ^= 1;
    c.add("cs/large-piece-data-flip", &csum, bad);
    // Extended samples with a changed digest: builds without `extended`
    // refuse them from the header, before the checksum (B20).
    let ui = info(32, 24, Channels::Gray, SampleType::U32);
    let upx = pixels(&ui, 33);
    let wide = lossless(
        upx.clone(),
        ui.clone(),
        enc(
            &upx,
            &ui,
            &Metadata::default(),
            &imij::ImiEncoderConfig::default().with_checksum(true),
        ),
    );
    let (_, wb) = common::checksum::csum(&wide.file);
    let mut bad = wide.file.clone();
    bad[wb - 1] ^= 1;
    c.add("cs/u32-digest-flip", &wide, bad);
    // A page or frame that is itself a document (B16).
    let inner = FrameEncoder::animation(
        pi.clone(),
        Metadata::default(),
        FrameConfig::new(imij::ImiEncoderConfig::default()),
    );
    let inner = {
        let mut e = inner;
        e.push_frame(
            &pixels(&pi, 34),
            pi.row_bytes(),
            FrameDuration::from_millis(10),
        )
        .unwrap();
        e.push_frame(
            &pixels(&pi, 35),
            pi.row_bytes(),
            FrameDuration::from_millis(10),
        )
        .unwrap();
        e.finish_legacy().unwrap()
    };
    let nested_doc = framed(pi.clone(), replace_frame(&uniform, &inner, 0));
    c.base("fr/page-holds-frames", &nested_doc);
    let mi = info(32, 24, Channels::Rgb, SampleType::U8);
    let inner_rgb = {
        let mut e = FrameEncoder::animation(
            mi.clone(),
            Metadata::default(),
            FrameConfig::new(imij::ImiEncoderConfig::default()),
        );
        e.push_frame(
            &pixels(&mi, 36),
            mi.row_bytes(),
            FrameDuration::from_millis(10),
        )
        .unwrap();
        e.push_frame(
            &pixels(&mi, 37),
            mi.row_bytes(),
            FrameDuration::from_millis(10),
        )
        .unwrap();
        e.finish_legacy().unwrap()
    };
    c.add(
        "fr/mixed-page-holds-frames",
        &mixed_plain,
        replace_frame(&mixed_plain.file, &inner_rgb, 0),
    );

    // -- limits --
    let px_count = rgb.info.width as u64 * rgb.info.height as u64;
    let bytes = rgb.info.checked_buffer_len().unwrap() as u64;
    let lim = |p: u64, b: u64| {
        let mut limits = Limits::DEFAULT;
        limits.max_pixels = p;
        limits.max_bytes = b;
        limits
    };
    c.add_limited("lm/pixels-at-limit", &rgb, f.clone(), lim(px_count, bytes));
    c.add_limited(
        "lm/pixels-over",
        &rgb,
        f.clone(),
        lim(px_count - 1, u64::MAX),
    );
    c.add_limited("lm/bytes-over", &rgb, f.clone(), lim(u64::MAX, bytes - 1));
    // The same bound on a lossy file: the whole output, at and one byte over.
    let ly_bytes = ly.info.checked_buffer_len().unwrap() as u64;
    let (name_at, name_over) = ("lm/lossy-bytes-at-limit", "lm/lossy-bytes-over");
    c.add_limited(name_at, &ly, ly.file.clone(), lim(u64::MAX, ly_bytes));
    c.add_limited(name_over, &ly, ly.file.clone(), lim(u64::MAX, ly_bytes - 1));
    c.add_limited(
        "lm/bytes-over-one-row-fits",
        &rgb,
        f.clone(),
        lim(u64::MAX, rgb.info.row_bytes() as u64 * 2),
    );
    c.add_limited(
        "lm/pixels-over-and-corrupt",
        &rgb,
        with_data(f, &ll_payload[..ll_payload.len() / 2]),
        lim(px_count - 1, u64::MAX),
    );
    // Over the limits and with a changed digest: one error on every route,
    // the limit first (B6).
    let mut bad = csum.file.clone();
    let (_, cb) = common::checksum::csum(&bad);
    bad[cb - 1] ^= 1;
    c.add_limited(
        "lm/pixels-over-and-mismatch",
        &csum,
        bad,
        lim(px_count - 1, u64::MAX),
    );
    let mut bad = csum.file.clone();
    bad[common::checksum::record_at(&csum.file)] = 5;
    c.add_limited(
        "lm/pixels-over-and-bad-record",
        &csum,
        bad,
        lim(px_count - 1, u64::MAX),
    );
    // Page one (gray8, 960 bytes) fits; pages two and three do not (B17).
    c.add_limited(
        "lm/later-page-over",
        &mixed_plain,
        mixed_plain.file.clone(),
        lim(u64::MAX, 1000),
    );

    review_cases(&mut c);
    capability_cases(&mut c, &mixed_plain, &anim);
    c
}

/// The feature set as a complete admission check (SPEC 0.1, 1.0): mask
/// bits at and above 64, gated syntax whose feature the file does not
/// declare (lossy head records 11 and 27), and frames and pages that need
/// a feature the outer header lacks (`Corrupt` when the document opens).
fn capability_cases(c: &mut Cases, mixed: &Base, anim: &Base) {
    let i = info(40, 24, Channels::Rgb, SampleType::U8);
    let px = pixels(&i, 50);
    let rgb = lossless(
        px.clone(),
        i.clone(),
        enc(&px, &i, &Metadata::default(), &lossless_cfg()),
    );
    // Feature id 64 (a tenth byte of 2) and id 300 (43 bytes).
    let mut bit64 = vec![0x80u8; 9];
    bit64.push(0x02);
    c.add(
        "cap/feature-bit-64",
        &rgb,
        common::with_feature_mask_bytes(&rgb.file, Some(&bit64)),
    );
    let mut bit300 = vec![0x80u8; 42];
    bit300.push(0x40);
    c.add(
        "cap/feature-bit-300",
        &rgb,
        common::with_feature_mask_bytes(&rgb.file, Some(&bit300)),
    );
    // Declaring an implied feature is over-declaring: the same image.
    c.add(
        "cap/implied-declared",
        &rgb,
        with_feature_mask(&rgb.file, Some(SAMPLES_32 | LOSSY)),
    );

    // An 8-bit lossy file above quality 100 with an i32 DC image declares
    // `samples_32` in every build: full-range content at q120
    // (as in tests/mixed_pages.rs).
    let fi = info(64, 64, Channels::Rgb, SampleType::U8);
    let full: Vec<u8> = (0..fi.row_bytes() * 64)
        .map(|k| {
            if (k / 24) % 2 == 0 {
                255
            } else {
                (k * 7 % 256) as u8
            }
        })
        .collect();
    let dc32_cfg =
        imij::ImiEncoderConfig::lossy(120.0).tune_lossy(|t| t.color = imij::LossyColor::YCoCg);
    match dc32_cfg.encode_legacy_samples(
        imij::ImageRef::new(&full, fi.row_bytes(), &fi),
        &Metadata::default(),
    ) {
        Ok(f) => {
            let mask = common::feature_mask(&f);
            assert_ne!(mask & SAMPLES_32, 0, "an i32 DC image declares samples_32");
            let dc32 = lossy(fi.clone(), f);
            c.base("cap/lossy-dc32", &dc32);
            c.add(
                "cap/lossy-dc32-undeclared",
                &dc32,
                with_feature_mask(&dc32.file, Some(mask & !SAMPLES_32)),
            );
        }
        Err(e) => {
            c.absent("cap/lossy-dc32", e.clone());
            c.absent("cap/lossy-dc32-undeclared", e);
        }
    }

    // Channel layouts (lossy head record 27) declare `channel_layouts`.
    let half =
        imij::LossyChannelLayout::new(imij::LossyTransform::Dct8, imij::ChannelSampling::HalfBoth);
    let cfg = lossy_cfg()
        .tune_lossy(|t| t.channel_layouts = [imij::LossyChannelLayout::default(), half, half]);
    let layouts = lossy(i.clone(), enc_lossy(&px, &i, &Metadata::default(), &cfg));
    let mask = common::feature_mask(&layouts.file);
    assert_ne!(
        mask & CHANNEL_LAYOUTS,
        0,
        "record 27 declares channel_layouts"
    );
    c.base("cap/lossy-layouts", &layouts);
    let d = data_payload(&layouts.file);
    let p = common::payload(&layouts.file[d.clone()], &i, true);
    let rec = p.head_record(27).expect("channel layouts record");
    let mut f = layouts.file.clone();
    let id_at = d.start + rec.start - common::varint_bytes(rec.len()).len() - 1;
    assert_eq!(f[id_at], 27);
    f[id_at] = 19;
    c.add("cap/lossy-layouts-retired-id", &layouts, f);
    c.add(
        "cap/lossy-layouts-undeclared",
        &layouts,
        with_feature_mask(&layouts.file, Some(mask & !CHANNEL_LAYOUTS)),
    );

    // Mixed pages: page one is lossless and page two lossy, so the outer
    // mask declares `lossy`; without it the document is corrupt at open.
    let outer = common::feature_mask(&mixed.file);
    assert_ne!(outer & LOSSY, 0, "a lossy page under a lossless page one");
    c.add(
        "cap/mixed-lossy-undeclared",
        mixed,
        with_feature_mask(&mixed.file, Some(outer & !LOSSY)),
    );
    // A 32-bit page after an 8-bit page one: `samples_32` in the outer mask.
    let mut m = FrameEncoder::mixed_pages(FrameConfig::new(imij::ImiEncoderConfig::default()));
    let first = info(24, 16, Channels::Gray, SampleType::U8);
    for pi in [first.clone(), info(20, 12, Channels::Gray, SampleType::U32)] {
        m.push_page_with(
            &pixels(&pi, 51),
            pi.row_bytes(),
            &pi,
            &Metadata::default(),
            &imij::ImiEncoderConfig::default(),
        )
        .unwrap();
    }
    let wide = framed(first, m.finish_legacy().unwrap());
    let outer = common::feature_mask(&wide.file);
    assert_ne!(outer & SAMPLES_32, 0, "a 32-bit page declares samples_32");
    c.base("cap/mixed-u32", &wide);
    c.add(
        "cap/mixed-u32-undeclared",
        &wide,
        with_feature_mask(&wide.file, Some(outer & !SAMPLES_32)),
    );
    // An animation whose second frame is lossy under a lossless frame 0:
    // valid with `lossy` in the outer mask, corrupt at open without it.
    let lossy_frame = enc_lossy(
        &pixels(&anim.info, 52),
        &anim.info,
        &Metadata::default(),
        &lossy_cfg(),
    );
    let (fa, fb) = find(&anim.file, b"fRAM").unwrap();
    let mut payload = vec![0, 0, 0, 1, 0];
    payload.extend_from_slice(&lossy_frame);
    let swapped = [
        &anim.file[..fa],
        &chunk(b"fRAM", &payload),
        &anim.file[fb..],
    ]
    .concat();
    // The outer mask covers the lossy frame's own mask (seed tables,
    // presets) in both; only `lossy` differs.
    let outer = common::feature_mask(&anim.file) | common::feature_mask(&lossy_frame);
    assert_eq!(outer & LOSSY, 0);
    let declared = framed(
        anim.info.clone(),
        with_feature_mask(&swapped, Some(outer | LOSSY)),
    );
    assert!(declared.frames.is_some(), "the declared animation decodes");
    c.base("cap/anim-lossy-frame", &declared);
    c.add(
        "cap/anim-lossy-frame-undeclared",
        &declared,
        with_feature_mask(&swapped, Some(outer)),
    );
}

/// A chunk header claiming `len` payload bytes (none follow).
/// The unit table forms of SPEC 2.7 (head record 12 and the trailing `uIDX`
/// chunk) on lossless and lossy files of one and of several units, and
/// their hostile forms. Every form decodes to the same pixels on every
/// route; every index a route reads is checked against the records.
fn unit_table_cases(c: &mut Cases, bases: [&Base; 5]) {
    use common::unit_table::{
        Form, append_index, head_record, index_chunk, reopened, spans, with_form, with_last_chunk,
        with_reservation,
    };
    let [ll1, ll, ly1, ly, lya] = bases;
    for (fam, b) in [
        ("ll1", ll1),
        ("ll", ll),
        ("ly1", ly1),
        ("ly", ly),
        ("lya", lya),
    ] {
        let f = &b.file;
        for (name, form) in [
            ("trailing", Form::Trailing),
            ("reserved4", Form::Reserved(4)),
            ("reserved3", Form::Reserved(3)),
            ("empty", Form::Empty(4)),
            ("tail", Form::Tail(4)),
        ] {
            let file = with_form(f, form);
            c.add(&format!("ut/{fam}-{name}"), b, file.clone());
            if matches!(form, Form::Trailing | Form::Tail(_)) {
                // A one-pass writer's open DATA, delimited by its records.
                c.add(&format!("ut/{fam}-{name}-open"), b, reopened(&file));
            }
        }
    }
    // Hostile forms, of the multi-unit lossless and lossy files.
    for (fam, b) in [("ll", ll), ("ly", ly)] {
        let f = &b.file;
        let s = spans(f);
        let d = data_payload(f).len() as u64;
        let trailing = with_form(f, Form::Trailing);
        let td = data_payload(&trailing).len() as u64;
        // Two different neighbouring spans swapped: the sum holds.
        let i = (0..s.len() - 1).find(|&i| s[i] != s[i + 1]).unwrap();
        let mut swapped = s.clone();
        swapped.swap(i, i + 1);
        let forged = index_chunk(&swapped, td);
        let index = |spans: &[u64], len: u64| with_last_chunk(&trailing, &index_chunk(spans, len));
        c.add(&format!("ut/{fam}-index-swapped"), b, index(&swapped, td));
        let mut sum = s.clone();
        *sum.last_mut().unwrap() += 1;
        c.add(&format!("ut/{fam}-index-sum"), b, index(&sum, td));
        let mut small = s.clone();
        small[0] = 1;
        c.add(&format!("ut/{fam}-index-entry-small"), b, index(&small, td));
        c.add(&format!("ut/{fam}-index-few"), b, index(&s[1..], td));
        // Entries far beyond 10 bytes each for the count: bounded first.
        let many = vec![2u64; s.len() * 10 + 1];
        c.add(&format!("ut/{fam}-index-too-long"), b, index(&many, td));
        c.add(&format!("ut/{fam}-index-other-data"), b, index(&s, td + 1));
        c.add(
            &format!("ut/{fam}-index-other-data-open"),
            b,
            reopened(&index(&s, td - 1)),
        );
        c.add(
            &format!("ut/{fam}-index-swapped-open"),
            b,
            reopened(&index(&swapped, td)),
        );
        // Cut short: the footer no longer names the chunk.
        c.add(
            &format!("ut/{fam}-index-truncated"),
            b,
            trailing[..trailing.len() - 1].to_vec(),
        );
        let open = reopened(&trailing);
        c.add(
            &format!("ut/{fam}-index-truncated-open"),
            b,
            open[..open.len() - 1].to_vec(),
        );
        // A forged index the footer does not name: no index.
        let mut tag = index(&swapped, td);
        *tag.last_mut().unwrap() = b'Y';
        c.add(&format!("ut/{fam}-index-footer-tag"), b, tag);
        let mut len = index(&swapped, td);
        let n = len.len();
        len[n - 12] ^= 1;
        c.add(&format!("ut/{fam}-index-footer-len"), b, len);
        // Not the last chunk: no index, whatever it holds.
        c.add(
            &format!("ut/{fam}-index-not-last"),
            b,
            [&index(&swapped, td)[..], &chunk(b"vnDs", b"after")].concat(),
        );
        c.add(
            &format!("ut/{fam}-index-twice"),
            b,
            [&trailing[..], &index_chunk(&s, td)].concat(),
        );
        c.add(
            &format!("ut/{fam}-index-before-data"),
            b,
            before_data(&with_reservation(f, Some(&[1])), &index_chunk(&s, td)),
        );
        // Check tail with no index (stripped): readers walk.
        let tail = with_form(f, Form::Tail(4));
        c.add(
            &format!("ut/{fam}-tail-no-index"),
            b,
            with_last_chunk(&tail, &[]),
        );
        c.add(
            &format!("ut/{fam}-trailing-no-index"),
            b,
            with_last_chunk(&trailing, &[]),
        );
        // A head hint, a filled reservation or an empty one next to a
        // forged index: the index is not read.
        c.add(
            &format!("ut/{fam}-hint-and-index"),
            b,
            [&f[..], &forged].concat(),
        );
        c.add(
            &format!("ut/{fam}-filled-and-index"),
            b,
            [&with_form(f, Form::Reserved(4))[..], &forged].concat(),
        );
        c.add(
            &format!("ut/{fam}-empty-and-index"),
            b,
            [&with_form(f, Form::Empty(4))[..], &forged].concat(),
        );
        c.add(
            &format!("ut/{fam}-hint-and-index-valid"),
            b,
            [&f[..], &index_chunk(&s, d)].concat(),
        );
        // Malformed reservations.
        let filled = with_form(f, Form::Reserved(4));
        let r = head_record(&filled, common::H_UNIT_RESERVE).unwrap();
        let mut long = filled[r.clone()].to_vec();
        long.extend_from_slice(&[2, 0, 0, 0]);
        c.add(
            &format!("ut/{fam}-reserve-long"),
            b,
            with_reservation(f, Some(&long)),
        );
        let mut width = filled[r.clone()].to_vec();
        width[1] = 9;
        c.add(
            &format!("ut/{fam}-reserve-width"),
            b,
            with_reservation(f, Some(&width)),
        );
        let mut entry = filled[r].to_vec();
        entry[2..6].copy_from_slice(&1u32.to_le_bytes());
        c.add(
            &format!("ut/{fam}-reserve-entry-small"),
            b,
            with_reservation(f, Some(&entry)),
        );
        c.add(
            &format!("ut/{fam}-reserve-state"),
            b,
            with_reservation(f, Some(&[3, 0])),
        );
        c.add(
            &format!("ut/{fam}-reserve-padding"),
            b,
            append_index(&with_reservation(f, Some(&[1, 0, 7]))),
        );
        // Records 2 and 4 in one head.
        let h = imij::header(f).unwrap();
        let range = data_payload(f);
        let pd = &f[range.clone()];
        let p = common::payload(pd, &h.info, h.coding == imij::Coding::Lossy);
        let both = common::with_head_record(pd, &p, common::H_UNIT_RESERVE, Some(&[0]));
        c.add(
            &format!("ut/{fam}-hint-and-reserve"),
            b,
            with_data(f, &both),
        );
    }
}

/// A checksummed trailing-index file, its table before `DATA` (as this
/// library writes it) and last but the index (as a streaming writer writes
/// it, SPEC 1.4), and each with one entry byte of the index flipped: a
/// mismatching piece explains the forged index on every verifying route.
fn unit_table_checksum_cases(c: &mut Cases, csum: &Base) {
    use common::checksum::{Place, record_at, strip, with_table};
    use common::unit_table::{Form, with_form};
    let f = &csum.file;
    let k = f[record_at(f)];
    let base = with_form(&strip(f), Form::Trailing);
    let index_at = common::chunks(&base).last().unwrap().1;
    for (name, place) in [
        ("before-data", Place::BeforeData),
        ("last", Place::Chunk(common::chunks(&base).len() - 1)),
    ] {
        let sealed = with_table(&base, k, 8, place);
        c.add(&format!("ut/csum-trailing-{name}"), csum, sealed.clone());
        // The first entry's byte, past the index chunk's tag and length
        // (the table moves it when it sits before it).
        let at = common::chunks(&sealed)
            .into_iter()
            .find(|ch| &ch.0 == b"uIDX")
            .unwrap()
            .1;
        assert!(at >= index_at);
        let mut flip = sealed;
        flip[at + 6] ^= 1;
        c.add(&format!("ut/csum-index-flip-{name}"), csum, flip);
    }
}

fn head_only(tag: &[u8; 4], len: u64) -> Vec<u8> {
    let mut c = tag.to_vec();
    put_varint(&mut c, len + 1);
    c
}

/// `file` with the tag of its first `from` chunk replaced in place (lengths
/// unchanged, so a DATA locator still matches).
fn renamed(file: &[u8], from: &[u8; 4], to: &[u8; 4]) -> Vec<u8> {
    let (a, _) = find(file, from).unwrap();
    let mut f = file.to_vec();
    f[a..a + 4].copy_from_slice(to);
    f
}

/// The first payload byte of the first `tag` chunk set to `v`.
fn with_first_payload_byte(file: &[u8], tag: &[u8; 4], v: u8) -> Vec<u8> {
    let (a, _) = find(file, tag).unwrap();
    let p = a + 4 + varint(&file[a + 4..]).1;
    let mut f = file.to_vec();
    f[p] = v;
    f
}

/// An `mCMP` payload: identity `id` and a stored (method 0) record
/// declaring `declared` bytes and holding `body`.
fn mcmp_stored(id: &[u8; 4], declared: u64, body: &[u8]) -> Vec<u8> {
    let mut p = id.to_vec();
    p.push(0);
    put_varint(&mut p, declared);
    p.extend_from_slice(body);
    p
}

/// The milestone A review's witness cases (`rv/<group>-...`), built from
/// the public API: A, rule violations that keep the DATA locator valid
/// (the region reader's locate path); B, a chunk that breaks a rule its
/// header shows and runs past the end of the file; C, chunks between DATA
/// and a trailing table; D, one changed byte in a checksummed file that also
/// breaks the mixed-pages rule; E, the metadata remux's cap.
fn review_cases(c: &mut Cases) {
    use common::checksum::{Place, strip, with_table};
    let info = info(96, 80, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 40);
    let enc = |meta: &Metadata, checksum: bool| {
        let cfg = lossless_cfg().with_checksum(checksum);
        enc(&px, &info, meta, &cfg)
    };
    let base = |file: Vec<u8>| lossless(px.clone(), info.clone(), file);
    let rgb = base(enc(&metadata(), false));
    let plain = base(enc(&Metadata::default(), false));
    let csum = base(enc(&metadata(), true));
    let csum_plain = base(enc(&Metadata::default(), true));
    assert!(ext(&rgb.file, 4).is_some(), "a DATA locator");
    // `b` (a checksummed file) edited without its table, then sealed again
    // at `place`.
    let resealed = |b: &[u8], place: Place, edit: &dyn Fn(&[u8]) -> Vec<u8>| {
        with_table(&edit(&strip(b)), 10, 8, place)
    };
    let f = &rgb.file;

    c.base("rv/base-rgb", &rgb);
    for (name, to) in [
        ("exif", b"eXIf"),
        ("mdcv", b"mDCv"),
        ("clli", b"cLLI"),
        ("zzzz", b"ZZZZ"),
        ("badtag", b"v1Dr"),
        ("csum", b"cSUM"),
        ("pdat", b"pDAT"),
    ] {
        c.add(
            &format!("rv/a-loc-vndr-as-{name}"),
            &rgb,
            renamed(f, b"vnDr", to),
        );
    }
    let as_anim = renamed(f, b"vnDr", b"aNIM");
    c.add(
        "rv/a-loc-vndr-as-anim3",
        &rgb,
        with_first_payload_byte(&as_anim, b"aNIM", 3),
    );
    c.add(
        "rv/a-loc-vndr-as-anim9-signal",
        &rgb,
        with_feature_mask(
            &with_first_payload_byte(&as_anim, b"aNIM", 9),
            Some(MIXED_PAGES),
        ),
    );
    c.add("rv/a-loc-xmp-as-mcmp", &rgb, renamed(f, b"xMPp", b"mCMP"));

    let past = 1u64 << 30;
    for (name, tag) in [
        ("badtag", b"v1Dr"),
        ("zzzz", b"ZZZZ"),
        ("mdcv", b"mDCv"),
        ("dup-exif", b"eXIf"),
    ] {
        c.add(
            &format!("rv/b-{name}-past-end"),
            &rgb,
            before_data(f, &head_only(tag, past)),
        );
    }
    let huge = (2 * 16 * 1024 * 1024 + 64) as u64 + 1;
    c.add(
        "rv/b-mcmp-huge-before-data",
        &plain,
        before_data(&plain.file, &head_only(b"mCMP", huge)),
    );
    c.add(
        "rv/b-mcmp-huge-after-data",
        &plain,
        [&plain.file[..], &head_only(b"mCMP", huge)].concat(),
    );
    c.add(
        "rv/b-anim-after-data-past-end",
        &rgb,
        [&f[..], &head_only(b"aNIM", past)].concat(),
    );
    let signal = with_feature_mask(f, Some(MIXED_PAGES));
    c.add(
        "rv/b-mixed-signal-and-dup-after",
        &rgb,
        after_data(&signal, &chunk(b"eXIf", b"again")),
    );
    c.add("rv/b-mixed-signal-no-data", &rgb, without(&signal, b"DATA"));
    c.add(
        "rv/b-two-csum-before-data",
        &rgb,
        before_data(
            &before_data(f, &chunk(b"cSUM", b"one")),
            &chunk(b"cSUM", b"two"),
        ),
    );

    let cp = &csum_plain.file;
    let packed_xmp = {
        let m = Metadata {
            xmp: metadata().xmp,
            ..Default::default()
        };
        let p = imij::compress_metadata(&enc(&m, false), 1 << 20).unwrap();
        let (a, b) = find(&p, b"mCMP").unwrap();
        p[a..b].to_vec()
    };
    let last = |b: &[u8], after: &[u8]| resealed(b, Place::Last, &|x| after_data(x, after));
    c.add(
        "rv/c-csumlast-base",
        &csum_plain,
        resealed(cp, Place::Last, &|x| x.to_vec()),
    );
    c.add(
        "rv/c-csumlast-good-mcmp-after",
        &csum_plain,
        last(cp, &packed_xmp),
    );
    c.add(
        "rv/c-csumlast-bad-mcmp-after",
        &csum_plain,
        last(cp, &chunk(b"mCMP", &mcmp_stored(b"eXIf", 9, b"short"))),
    );
    c.add(
        "rv/c-csumlast-dup-exif-after",
        &csum,
        last(&csum.file, &chunk(b"eXIf", b"again")),
    );
    c.add(
        "rv/c-csumlast-anim-after",
        &csum_plain,
        last(cp, &chunk(b"aNIM", &[1, 0, 1, 1, 1])),
    );
    c.add(
        "rv/c-csumlast-bad-mdcv-after",
        &csum_plain,
        last(cp, &chunk(b"mDCv", &[0; 5])),
    );
    c.add(
        "rv/c-csumlast-zzzz-after",
        &csum_plain,
        last(cp, &chunk(b"ZZZZ", b"x")),
    );
    c.add(
        "rv/c-csumfirst-dup-exif-after",
        &csum,
        resealed(&csum.file, Place::BeforeData, &|x| {
            after_data(x, &chunk(b"eXIf", b"again"))
        }),
    );

    for (name, place) in [("first", Place::BeforeData), ("last", Place::Last)] {
        let good = resealed(cp, place, &|x| {
            before_data(x, &chunk(b"aNIM", &[1, 0, 1, 1, 1]))
        });
        let flipped = with_first_payload_byte(&good, b"aNIM", 3);
        c.add(
            &format!("rv/d-anim-kind-flip-table-{name}-unflipped"),
            &csum_plain,
            good,
        );
        c.add(
            &format!("rv/d-anim-kind-flip-table-{name}"),
            &csum_plain,
            flipped,
        );
    }
    c.add(
        "rv/d-exif-tag-flip-to-dup",
        &csum,
        renamed(&csum.file, b"vnDr", b"eXIf"),
    );

    c.add(
        "rv/e-mcmp-over-cap-and-corrupt",
        &plain,
        before_data(
            &plain.file,
            &chunk(b"mCMP", &mcmp_stored(b"eXIf", 2 << 20, b"abcde")),
        ),
    );
    c.add(
        "rv/e-mcmp-over-cap-valid",
        &plain,
        before_data(
            &plain.file,
            &chunk(b"mCMP", &mcmp_stored(b"eXIf", 2 << 20, &vec![7u8; 2 << 20])),
        ),
    );
}

#[test]
fn every_route_on_every_case() {
    let Cases(cases, absent) = cases();
    let mut observed = BTreeMap::new();
    let mut lines = Vec::new();
    for (name, why) in &absent {
        eprintln!("{name}: absent in this build ({why})");
    }
    let mut wrong = Vec::new();
    for case in &cases {
        let line = observe_all(case);
        for (route, o) in &line {
            // With verification off a changed byte may decode to other
            // pixels: no integrity guarantee, so only verifying routes fail.
            let verified = !route.ends_with("-off");
            // A reader meets a trailing table, or the end of a piece too
            // large to read ahead, after the rows (SPEC 1.4).
            let late_table = (case.name.contains("table-last")
                || case.name.contains("large-piece"))
                && route.contains("reader");
            if o == "PANIC"
                || (verified && o.starts_with("ok!"))
                || (verified && !late_table && o.ends_with("+rows!"))
            {
                wrong.push(format!("{} {route}: {o}", case.name));
            }
        }
        lines.push(format_line(&case.name, &line));
        observed.insert(case.name.clone(), line);
    }
    let build = build();
    let ptr32 = cfg!(target_pointer_width = "32");
    let path = format!(
        "{}/tests/matrix/{}.txt",
        env!("CARGO_MANIFEST_DIR"),
        if ptr32 { "ptr32" } else { build }
    );
    let default = parse_table(DEFAULT_TABLE);
    let build_overlay = parse_table(match build {
        "lab" => LAB_TABLE,
        "minimal" => MINIMAL_TABLE,
        _ => "",
    });
    let header = format!(
        "# Cross-route outcome matrix ({}; tests/matrix.rs). One line per case:\n\
         # name, the commonest outcome, then route=outcome for each route that differs.\n\
         # Routes: {}\n",
        if ptr32 {
            "32-bit targets, lines that differ from the 64-bit tables"
        } else {
            match build {
                "lab" => "lab build",
                "minimal" => "minimal build",
                _ => "default build",
            }
        },
        ROUTES.iter().map(|r| r.name).collect::<Vec<_>>().join(" ")
    );
    if std::env::var_os("IMIJ_MATRIX_BLESS").is_some() {
        let keep: Vec<&String> = if build == "default" && !ptr32 {
            lines.iter().collect()
        } else {
            // Overlays hold the lines that differ from the table they
            // replace: the build overlays from `default`, the 32-bit
            // table from what the same build reports on 64-bit (its
            // build overlay where it has the case, else `default`).
            lines
                .iter()
                .filter(|l| {
                    let name = l.split('\t').next().unwrap();
                    let o = &observed[name];
                    ROUTES.iter().any(|r| {
                        let base = if ptr32 {
                            expected(&build_overlay, name, r.name)
                                .or_else(|| expected(&default, name, r.name))
                        } else {
                            expected(&default, name, r.name)
                        };
                        base != Some(o[r.name].as_str())
                    })
                })
                .collect()
        };
        let mut text = header;
        for l in keep {
            text.push_str(l);
            text.push('\n');
        }
        for (name, _) in &absent {
            assert!(
                build != "default" || ptr32,
                "{name}: every case builds in the default build"
            );
            text.push_str(&format!("{name}\tabsent\n"));
        }
        std::fs::write(&path, text).unwrap();
        eprintln!("blessed {path}");
    } else {
        let overlay = parse_table(PTR32_TABLE);
        // The 32-bit table may only carry a decode route's `TooLarge`
        // where the same build's 64-bit tables report a structural error;
        // a header route or any other outcome belongs in the build's own
        // tables (or is a code bug).
        for (name, (common, ex)) in &overlay {
            for r in ROUTES {
                let got = ex.get(r.name).unwrap_or(common);
                let base = expected(&build_overlay, name, r.name)
                    .or_else(|| expected(&default, name, r.name));
                if Some(got.as_str()) != base {
                    let structural = base.is_some_and(|b| {
                        b.starts_with("Corrupt:") || b.starts_with("Unsupported:")
                    });
                    assert!(
                        r.class != Class::Header && got == "TooLarge" && structural,
                        "matrix/ptr32.txt: {name} {} = {got} may differ only on a \
                         decode route reporting TooLarge where the 64-bit tables \
                         report a structural error (they say {})",
                        r.name,
                        base.unwrap_or("(no line)")
                    );
                }
            }
        }
        let mut diffs = Vec::new();
        for case in &cases {
            let o = &observed[&case.name];
            let table = if overlay.contains_key(&case.name) {
                &overlay
            } else if build_overlay.contains_key(&case.name) {
                &build_overlay
            } else {
                &default
            };
            for r in ROUTES {
                let want = expected(table, &case.name, r.name);
                if want != Some(o[r.name].as_str()) {
                    diffs.push(format!(
                        "{} {}: expected {}, got {}",
                        case.name,
                        r.name,
                        want.unwrap_or("(no line)"),
                        o[r.name]
                    ));
                }
            }
        }
        for name in default
            .keys()
            .chain(build_overlay.keys())
            .chain(overlay.keys())
        {
            let gone = absent.iter().any(|a| &a.0 == name);
            let listed = [&build_overlay, &overlay]
                .iter()
                .any(|t| t.get(name).is_some_and(|l| l.0 == "absent"));
            if gone != listed || (!gone && !observed.contains_key(name)) {
                diffs.push(format!("{name}: in the table, not a case of this build"));
            }
        }
        assert!(
            diffs.is_empty(),
            "{} outcomes differ from tests/matrix/{}.txt (IMIJ_MATRIX_BLESS=1 rewrites it):\n{}",
            diffs.len(),
            if ptr32 { "ptr32" } else { build },
            diffs.join("\n")
        );
    }
    assert!(
        wrong.is_empty(),
        "wrong pixels or panics:\n{}",
        wrong.join("\n")
    );
}
