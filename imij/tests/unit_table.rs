//! Retained V2/V3 wire grammar controls use the explicit legacy producer.
//! The unit table forms of SPEC 2.7 on large multi-unit files: how many
//! batches and ranges the region reader needs to open each and to decode
//! one region (head hint; reservation filled; check tail with a trailing
//! `uIDX` index, bounded and open `DATA`; reservation left empty, which is
//! walked), and, in lab builds, the library's own writer path against the
//! test-side writers of `common::unit_table`.

mod common;

use common::data_payload;
use common::unit_table::{Form, reopened, spans, with_form};
use imij::planes::{ReadAt, Rect};
use imij::region::RegionReader;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

/// A source that counts what a remote store would serve: batches (one round
/// trip each when its ranges are fetched concurrently), ranges (one round
/// trip each when they are fetched one at a time) and bytes.
struct Counting<'a> {
    bytes: &'a [u8],
    batches: u64,
    ranges: u64,
    read: u64,
}

impl ReadAt for Counting<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.bytes.len() as u64)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.read_many_at(&mut [(offset, out)])
    }
    fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.batches += 1;
        for (offset, out) in reqs.iter_mut() {
            let a = *offset as usize;
            out.copy_from_slice(&self.bytes[a..a + out.len()]);
            self.ranges += 1;
            self.read += out.len() as u64;
        }
        Ok(())
    }
}

fn noise(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let n = info.row_bytes() * info.height as usize;
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let (w, nch) = (info.width as usize, info.channels.count());
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let x = i / nch % w;
            let y = i / nch / w;
            ((x * 3 + y * 5) as u32 % 160 + s % 64) as u8
        })
        .collect()
}

/// What opening a file and decoding one region cost.
#[derive(Debug, PartialEq, Eq)]
struct Cost {
    indexed: bool,
    open: (u64, u64, u64),
    region: (u64, u64, u64),
}

fn cost(file: &[u8], rect: Rect, want: &[u8], info: &ImageInfo) -> Cost {
    let src = Counting {
        bytes: file,
        batches: 0,
        ranges: 0,
        read: 0,
    };
    let mut rr = RegionReader::open(src).unwrap();
    let indexed = rr.has_unit_table();
    let s = rr.read_stats();
    let open = (s.batches, s.reads, s.bytes);
    rr.reset_read_stats();
    let bpp = info.row_bytes() / info.width as usize;
    let rb = rect.width() as usize * bpp;
    let mut out = vec![0u8; rb * rect.height() as usize];
    rr.decode_region(rect, &mut out, rb).unwrap();
    for (i, row) in out.chunks(rb).enumerate() {
        let a = (rect.y() as usize + i) * info.row_bytes() + rect.x() as usize * bpp;
        assert_eq!(row, &want[a..a + rb], "region row {i}");
    }
    let s = rr.read_stats();
    let region = (s.batches, s.reads, s.bytes);
    // The source saw what the reader counted.
    let src = rr.into_inner();
    assert_eq!(
        (src.batches, src.ranges, src.read),
        (open.0 + region.0, open.1 + region.1, open.2 + region.2)
    );
    Cost {
        indexed,
        open,
        region,
    }
}

/// A large file, its decoded pixels and the region read from it.
struct Large {
    name: &'static str,
    file: Vec<u8>,
    info: ImageInfo,
    px: Vec<u8>,
    rect: Rect,
}

/// The large files.
fn large() -> Vec<Large> {
    let mut out = Vec::new();
    // 1024 units of 16 rows x 256 columns: the index fits the last 64 KiB.
    let info = ImageInfo::new(2048, 2048, Channels::Rgb, SampleType::U8);
    let px = noise(&info, 1);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg = cfg.with_column_groups(Some(256));
        cfg
    };
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    out.push(Large {
        name: "lossless 2048x2048 RGB, 1024 units",
        file,
        info,
        px,
        rect: Rect::new(896, 896, 256, 256),
    });
    // 65,536 units of one row x 256 columns: a 128 KiB index, longer than
    // the tail window.
    let info = ImageInfo::new(4096, 4096, Channels::Gray, SampleType::U8);
    let px = noise(&info, 2);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(1);
        });
        cfg = cfg.with_column_groups(Some(256));
        cfg
    };
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    out.push(Large {
        name: "lossless 4096x4096 gray, 65536 units",
        file,
        info,
        px,
        rect: Rect::new(1920, 1920, 256, 256),
    });
    let info = ImageInfo::new(2048, 2048, Channels::Rgb, SampleType::U8);
    let src = noise(&info, 3);
    let cfg = imij::ImiEncoderConfig::lossy(70.0).tune_lossy(|t| t.stripe_block_rows = 1);
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&src, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let px = imij::decode(&file).unwrap().1;
    out.push(Large {
        name: "lossy 2048x2048 RGB, 256 stripes",
        file,
        info,
        px,
        rect: Rect::new(896, 896, 256, 256),
    });
    out
}

/// Opening and one 256 x 256 region per form: every indexed form opens
/// in a few batches and decodes the region in one; an empty reservation is
/// walked, one batch per record head. The counts are printed for the record
/// (`--nocapture`) and pinned below.
#[test]
fn region_reads_per_form() {
    let forms: [(Option<Form>, &str, bool); 6] = [
        (None, "head hint (record 10)", true),
        (Some(Form::Reserved(3)), "reservation filled (w=3)", true),
        (Some(Form::Reserved(4)), "reservation filled (w=4)", true),
        (Some(Form::Trailing), "trailing index", true),
        (Some(Form::Tail(4)), "reservation + check tail", true),
        (Some(Form::Empty(4)), "reservation empty (walk)", false),
    ];
    let mut seen = Vec::new();
    for Large {
        name,
        file,
        info,
        px,
        rect,
    } in large()
    {
        let records = spans(&file).len() as u64;
        eprintln!("{name}: {records} main records, {} bytes", file.len());
        eprintln!(
            "  {:<38} | open: batches ranges bytes | region: batches ranges bytes",
            "form"
        );
        for (form, label, indexed) in forms {
            let f = match form {
                None => file.clone(),
                Some(form) => with_form(&file, form),
            };
            let mut variants = vec![(label.to_string(), f.clone())];
            if matches!(form, Some(Form::Trailing | Form::Tail(_))) {
                variants.push((format!("{label}, open DATA"), reopened(&f)));
            }
            for (label, f) in variants {
                let c = cost(&f, rect, &px, &info);
                eprintln!(
                    "  {label:<38} | {:>5} {:>6} {:>9} | {:>3} {:>6} {:>9}",
                    c.open.0, c.open.1, c.open.2, c.region.0, c.region.1, c.region.2
                );
                assert_eq!(c.indexed, indexed, "{name} {label}");
                assert_eq!(c.region.0, 1, "{name} {label}: region batches");
                seen.push((name, label, c.open.0));
            }
        }
    }
    let got: Vec<u64> = seen.iter().map(|s| s.2).collect();
    assert_eq!(got, OPEN_BATCHES, "{seen:#?}");
}

/// Open batches of `region_reads_per_form`, file by file, form by form
/// (measured 2026-10-11): one more for the trailing index than for the hint
/// (the end of the file), none more when the hint itself lies past the
/// first 64 KiB (4 each for 65,536 units); a reservation that falls back to
/// check tail keeps its padding in the head (6); walking costs one batch per
/// record head.
#[rustfmt::skip]
const OPEN_BATCHES: [u64; 24] = [
    2, 2, 2, 3, 3, 3, 3, 1025,
    4, 4, 4, 4, 4, 6, 6, 65_539,
    2, 2, 2, 3, 3, 3, 3, 289,
];

/// The trailing index bounds its own length by the geometry before the
/// rest of it is fetched: a footer naming a chunk far longer than any valid
/// index costs one header read and is `Corrupt`, on every route.
#[test]
fn a_long_index_is_refused_before_it_is_fetched() {
    use std::cell::Cell;
    /// Counts the bytes it serves into a cell that outlives the reader.
    struct Served<'a>(&'a [u8], &'a Cell<u64>);
    impl ReadAt for Served<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.0.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            let a = offset as usize;
            out.copy_from_slice(&self.0[a..a + out.len()]);
            self.1.set(self.1.get() + out.len() as u64);
            Ok(())
        }
    }
    let info = ImageInfo::new(256, 256, Channels::Gray, SampleType::U8);
    let px = noise(&info, 4);
    let cfg = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(16);
        });
        cfg
    };
    let file = cfg
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let trailing = with_form(&file, Form::Trailing);
    // The 16 entries, then 200 Ki more before the footer.
    let mut many = spans(&file);
    many.extend(std::iter::repeat_n(2, 200 * 1024));
    let d = data_payload(&trailing).len() as u64;
    // The index alone is longer than the two windows the reader reads.
    let index = common::unit_table::index_chunk(&many, d);
    assert!(index.len() > 2 * 65_536);
    let forged = common::unit_table::with_last_chunk(&trailing, &index);
    let served = Cell::new(0);
    assert_eq!(
        RegionReader::open(Served(&forged, &served)).err(),
        Some(Error::Corrupt("unit table length"))
    );
    // The first and the last 64 KiB and one chunk header: never the index.
    assert!(served.get() <= 2 * 65_536 + 14, "{}", served.get());
    assert_eq!(
        imij::decode(&forged).err(),
        Some(Error::Corrupt("unit table length"))
    );
}

/// The library's writer path (`lab::set_unit_table_form`) writes the same
/// bytes as the test-side writers for every form: the head record, the
/// fill or mark in place, the trailing index and the DATA locator.
#[cfg(all(feature = "lab", feature = "std"))]
#[test]
fn the_lab_writer_matches_the_test_side_writer() {
    use imij::lab::{UnitTableForm, set_unit_table_form};
    let meta = Metadata {
        exif: Some(vec![7; 3000]),
        ..Metadata::default()
    };
    type Enc = Box<dyn Fn() -> Vec<u8>>;
    let mut images: Vec<(&str, Enc)> = Vec::new();
    for (w, h, ch, rows) in [
        (96, 80, Channels::Rgb, 8),
        (300, 40, Channels::Rgba, 16),
        (40, 30, Channels::Gray, 64),
    ] {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = noise(&info, w);
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(rows);
            });
            cfg = cfg.with_column_groups((w >= 300).then_some(64));
            cfg
        };
        let meta = meta.clone();
        images.push((
            "lossless",
            Box::new(move || {
                cfg.encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
                    .unwrap()
            }),
        ));
    }
    for (w, h, ch) in [
        (96, 80, Channels::Rgb),
        (80, 48, Channels::Rgba),
        (16, 8, Channels::Rgb),
    ] {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = noise(&info, w + 1);
        let cfg = imij::ImiEncoderConfig::lossy(60.0).tune_lossy(|t| t.stripe_block_rows = 1);
        let meta = meta.clone();
        images.push((
            "lossy",
            Box::new(move || {
                cfg.encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
                    .unwrap()
            }),
        ));
    }
    for (name, enc) in &images {
        let hint = enc();
        for (lab, test) in [
            (UnitTableForm::Trailing, Form::Trailing),
            (UnitTableForm::Reserved(4), Form::Reserved(4)),
            (UnitTableForm::Reserved(3), Form::Reserved(3)),
            (UnitTableForm::ReservedEmpty(4), Form::Empty(4)),
            (UnitTableForm::ReservedTail(2), Form::Tail(2)),
        ] {
            set_unit_table_form(lab);
            let written = enc();
            set_unit_table_form(UnitTableForm::Hint);
            assert_eq!(written, with_form(&hint, test), "{name} {lab:?}");
        }
    }
}
