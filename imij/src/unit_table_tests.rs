//! The unit table's writer forms (SPEC 2.7) through every kind of decoder:
//! the hint, none, check tail with a trailing `uIDX` chunk, and a head
//! reservation filled, left empty or marked check tail. Every form decodes
//! to the same pixels on the slice, threaded, row-stream, reader and region
//! routes; a reservation is filled before the checksum covers it.

use super::*;
use crate::parse::records::{RESERVE_EMPTY, RESERVE_FILLED, RESERVE_TAIL, Reserve, UIDX};
use crate::planes::{Rect, SliceReader};
use crate::region::RegionReader;
use crate::{Channels, EncodeConfig, LossyConfig, Metadata, SampleType};

fn image(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let n = info.row_bytes() * info.height as usize;
    let mut s = 0x9E37_79B9u32 ^ seed;
    (0..n)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            ((i / 5) as u32 % 180 + s % 23) as u8
        })
        .collect()
}

/// An encoder of one test image under the thread's unit table form.
struct Image {
    name: &'static str,
    info: ImageInfo,
    px: Vec<u8>,
    lossy: Option<LossyConfig>,
    lossless: EncodeConfig,
    /// Main records of its payload.
    multi: bool,
}

impl Image {
    fn encode(&self, checksum: bool) -> Vec<u8> {
        let rb = self.info.row_bytes();
        match &self.lossy {
            Some(cfg) => {
                let cfg = LossyConfig {
                    checksum,
                    ..cfg.clone()
                };
                crate::encode_lossy(&self.px, rb, &self.info, &Metadata::default(), &cfg)
            }
            None => {
                let cfg = EncodeConfig {
                    checksum,
                    ..self.lossless.clone()
                };
                crate::encode(&self.px, rb, &self.info, &Metadata::default(), &cfg)
            }
        }
        .unwrap()
    }
}

fn images() -> Vec<Image> {
    let mk = |name, w, h, ch, lossy: Option<LossyConfig>, lossless, multi| {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        Image {
            name,
            px: image(&info, w * 31 + h),
            info,
            lossy,
            lossless,
            multi,
        }
    };
    let rows = |r| EncodeConfig {
        stripe_rows: Some(r),
        ..EncodeConfig::default()
    };
    let lossy = |sb| LossyConfig {
        stripe_block_rows: sb,
        ..LossyConfig::default()
    };
    vec![
        mk("lossless", 64, 48, Channels::Rgb, None, rows(8), true),
        mk(
            "lossless groups",
            300,
            20,
            Channels::Rgba,
            None,
            EncodeConfig {
                column_groups: Some(64),
                ..rows(8)
            },
            true,
        ),
        mk(
            "lossless one unit",
            20,
            10,
            Channels::Gray,
            None,
            EncodeConfig::default(),
            false,
        ),
        mk(
            "lossy",
            64,
            64,
            Channels::Rgb,
            Some(lossy(1)),
            EncodeConfig::default(),
            true,
        ),
        mk(
            "lossy alpha",
            48,
            40,
            Channels::Rgba,
            Some(lossy(1)),
            EncodeConfig::default(),
            true,
        ),
        mk(
            "lossy one unit",
            16,
            8,
            Channels::Rgb,
            Some(lossy(1)),
            EncodeConfig::default(),
            false,
        ),
    ]
}

const FORMS: [UnitTableForm; 8] = [
    UnitTableForm::Hint,
    UnitTableForm::None,
    UnitTableForm::Trailing,
    UnitTableForm::Reserved(4),
    UnitTableForm::Reserved(3),
    UnitTableForm::Reserved(1),
    UnitTableForm::ReservedEmpty(4),
    UnitTableForm::ReservedTail(4),
];

/// The file's chunks after its header: (tag, start, end).
fn chunks(file: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut r = ByteReader::new(file);
    crate::parse::header::read_header_sized(&mut r).unwrap();
    let mut out = Vec::new();
    while r.remaining() > 0 {
        let at = r.pos;
        let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
        r.bytes(len.unwrap() as usize).unwrap();
        out.push((tag, at, r.pos));
    }
    out
}

/// The `DATA` payload's head: its hint and reservation.
fn table_of(file: &[u8]) -> (Option<Vec<u64>>, Option<Reserve>) {
    let mut r = ByteReader::new(file);
    let hp = crate::parse::header::read_header_sized(&mut r).unwrap();
    let (_, len) = crate::parse::chunks::walk_to_data(&mut r, |_, _| {}).unwrap();
    let body = &file[r.pos..r.pos + len.unwrap() as usize];
    let mut head = crate::parse::payload::read_head(
        &hp.header,
        hp.declared,
        &mut ByteReader::new(body),
        &crate::parse::limits::Structure,
    )
    .unwrap();
    (head.take_hint(), head.reserve())
}

/// The spans of the main records of `file`'s `DATA` payload.
fn spans_of(file: &[u8]) -> Vec<u64> {
    let mut r = ByteReader::new(file);
    let hp = crate::parse::header::read_header_sized(&mut r).unwrap();
    let (_, len) = crate::parse::chunks::walk_to_data(&mut r, |_, _| {}).unwrap();
    let body = &file[r.pos..r.pos + len.unwrap() as usize];
    let mut br = ByteReader::new(body);
    let head = crate::parse::payload::read_head(
        &hp.header,
        hp.declared,
        &mut br,
        &crate::parse::limits::Structure,
    )
    .unwrap();
    let (map, stripes) = head.units();
    map.spans(&mut br, crate::parse::header::HEADER_VERSION, stripes, None)
        .unwrap()
}

/// The pixels of `file` through every decoder kind, each equal to `want`.
fn every_decoder(file: &[u8], want: &[u8], what: &str) {
    let (info, px) = crate::decode(file).unwrap();
    assert_eq!(px, want, "{what}: decode");
    let rb = info.row_bytes();
    let mut out = vec![0u8; want.len()];
    crate::decode_into_threads(file, &mut out, rb, 3).unwrap();
    assert_eq!(out, want, "{what}: threads");
    let mut s = crate::StreamDecoder::new(file).unwrap();
    let mut rows = Vec::new();
    let mut row = vec![0u8; rb];
    while s.next_row(&mut row).unwrap() {
        rows.extend_from_slice(&row);
    }
    assert_eq!(rows, want, "{what}: stream");
    let mut rows = Vec::new();
    crate::decode_reader(
        std::io::Cursor::new(file),
        &crate::Limits::DEFAULT,
        |_, _, r| {
            rows.extend_from_slice(r);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(rows, want, "{what}: reader");
    let mut rr = RegionReader::open(SliceReader::new(file)).unwrap();
    out.fill(0);
    rr.decode_region(Rect::new(0, 0, info.width, info.height), &mut out, rb)
        .unwrap();
    assert_eq!(out, want, "{what}: region");
    // A rectangle inside, in the region reader's own buffer.
    let (x, y, w, h) = (
        info.width / 3,
        info.height / 2,
        info.width / 2,
        info.height / 3,
    );
    let bpp = rb / info.width as usize;
    let mut sub = vec![0u8; w as usize * h as usize * bpp];
    rr.decode_region(Rect::new(x, y, w, h), &mut sub, w as usize * bpp)
        .unwrap();
    for (i, r) in sub.chunks(w as usize * bpp).enumerate() {
        let at = (y as usize + i) * rb + x as usize * bpp;
        assert_eq!(r, &want[at..at + r.len()], "{what}: region row {i}");
    }
}

/// Every form of every image decodes to the hint form's pixels on every
/// decoder kind, with and without a file checksum; the head and the chunks
/// carry what the form says, and the region reader finds the spans where
/// the form put them.
#[test]
fn every_form_decodes_alike_on_every_decoder() {
    // Reservations whose spans did not fit, which fell back to the tail.
    let mut fell_back = 0;
    for img in images() {
        let want = crate::decode(&img.encode(false)).unwrap().1;
        if img.lossy.is_none() {
            assert_eq!(want, img.px, "{}", img.name);
        }
        for form in FORMS {
            for checksum in [false, true] {
                let file = with_unit_table(form, || img.encode(checksum));
                let what = format!("{} {form:?} checksum {checksum}", img.name);
                let (hint, reserve) = table_of(&file);
                let last = chunks(&file).last().unwrap().0;
                let state = reserve.map(|r| r.state);
                // A span that does not fit the reservation's width: the
                // writer falls back to the tail.
                let wide = match form {
                    UnitTableForm::Reserved(w) => {
                        spans_of(&file).iter().any(|&e| e >> (8 * w) != 0)
                    }
                    _ => false,
                };
                fell_back += usize::from(wide);
                let tail = matches!(
                    form,
                    UnitTableForm::Trailing | UnitTableForm::ReservedTail(_)
                ) || wide;
                match form {
                    UnitTableForm::Hint => {
                        assert_eq!((hint.is_some(), state), (img.multi, None), "{what}")
                    }
                    UnitTableForm::None => assert_eq!((hint, state), (None, None), "{what}"),
                    _ if tail => assert_eq!((hint, state), (None, Some(RESERVE_TAIL)), "{what}"),
                    UnitTableForm::Reserved(_) => {
                        assert!(hint.is_some(), "{what}");
                        assert_eq!(state, Some(RESERVE_FILLED), "{what}");
                    }
                    _ => assert_eq!((hint, state), (None, Some(RESERVE_EMPTY)), "{what}"),
                }
                if form == UnitTableForm::Trailing {
                    // The 3-byte check-tail record: no reservation.
                    assert_eq!(reserve.unwrap().len, 1, "{what}");
                }
                assert_eq!(last == UIDX, tail, "{what}");
                every_decoder(&file, &want, &what);
                let rr = RegionReader::open(SliceReader::new(&file)).unwrap();
                let indexed = match form {
                    UnitTableForm::Hint => img.multi,
                    UnitTableForm::None | UnitTableForm::ReservedEmpty(_) => false,
                    _ => true,
                };
                assert_eq!(rr.has_unit_table(), indexed, "{what}");
                // Writers write the DATA locator for a head with record 10
                // or record 12, and the region reader uses it.
                let located = match form {
                    UnitTableForm::Hint => img.multi,
                    UnitTableForm::None => false,
                    _ => true,
                };
                assert_eq!(rr.used_locator(), located, "{what}");
            }
        }
    }
    assert!(fell_back > 0, "no reservation fell back to the tail");
}

/// Hint 10 and reservation 12 straddle the critical i32 DC declaration 11.
/// Exercise both writer orders with checksums and all decoder routes.
#[test]
fn unit_tables_with_i32_dc_keep_increasing_head_order() {
    let info = ImageInfo::new(64, 64, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..info.row_bytes() * 64)
        .map(|k| {
            if (k / 24) % 2 == 0 {
                255
            } else {
                (k * 7 % 256) as u8
            }
        })
        .collect();
    let cfg = LossyConfig {
        stripe_block_rows: 1,
        ..LossyConfig::new(120.0).with_color(crate::LossyColor::YCoCg)
    };
    let encode = |checksum| {
        crate::encode_lossy(
            &px,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &cfg.clone().with_checksum(checksum),
        )
        .unwrap()
    };
    let plain = with_unit_table(UnitTableForm::None, || encode(false));
    let want = crate::decode(&plain).unwrap().1;
    assert!(
        crate::header(&plain)
            .unwrap()
            .features
            .contains(crate::Feature::Samples32)
    );
    for form in [UnitTableForm::Hint, UnitTableForm::Reserved(4)] {
        for checksum in [false, true] {
            let file = with_unit_table(form, || encode(checksum));
            assert!(table_of(&file).0.is_some());
            every_decoder(&file, &want, "unit table with i32 DC");
        }
    }
}

/// A reservation filled before the checksum verifies; filled after the
/// checksum was computed over its zeros, the file is a checksum mismatch on
/// every route: the writer fills, then seals (SPEC 1.4).
#[test]
fn reservations_are_filled_before_the_checksum() {
    let img = &images()[0];
    let filled = with_unit_table(UnitTableForm::Reserved(4), || img.encode(true));
    let want = crate::decode(&img.encode(false)).unwrap().1;
    every_decoder(&filled, &want, "filled then sealed");
    // Sealed over the empty reservation, then filled in place: the bytes
    // the filled file has, under the empty file's digests.
    let empty = with_unit_table(UnitTableForm::ReservedEmpty(4), || img.encode(true));
    assert_eq!(empty.len(), filled.len());
    let changed: Vec<usize> = (0..empty.len())
        .filter(|&i| empty[i] != filled[i])
        .collect();
    let (data_at, data_end) = chunks(&filled)
        .iter()
        .find(|c| &c.0 == b"DATA")
        .map(|c| (c.1, c.2))
        .unwrap();
    let csum = chunks(&filled)
        .iter()
        .find(|c| &c.0 == b"cSUM")
        .map(|c| (c.1, c.2))
        .unwrap();
    let mut late = empty.clone();
    for &i in &changed {
        // Only the reservation and the digests differ.
        assert!(
            (data_at..data_end).contains(&i) || (csum.0..csum.1).contains(&i),
            "byte {i}"
        );
        if (data_at..data_end).contains(&i) {
            late[i] = filled[i];
        }
    }
    assert_eq!(
        crate::decode(&late).map(|_| ()),
        Err(Error::Corrupt("file checksum mismatch"))
    );
    let mut rr = RegionReader::open(SliceReader::new(&late)).unwrap();
    let info = img.info.clone();
    let mut out = vec![0u8; want.len()];
    assert_eq!(
        rr.decode_region(
            Rect::new(0, 0, info.width, info.height),
            &mut out,
            info.row_bytes()
        ),
        Err(Error::Corrupt("file checksum mismatch"))
    );
}

/// A multi-frame file: the primary's trailing index is the last chunk,
/// after every frame, and the primary decodes alike on every route.
#[test]
fn frame_files_end_with_the_primary_index() {
    use crate::{FrameConfig, FrameEncoder};
    let info = ImageInfo::new(32, 32, Channels::Rgb, SampleType::U8);
    let cfg = FrameConfig::new(
        crate::ImiEncoderConfig::default().tune_lossless(|t| t.stripe_rows = Some(8)),
    );
    for form in [UnitTableForm::Trailing, UnitTableForm::ReservedTail(2)] {
        let file = with_unit_table(form, || {
            let mut e = FrameEncoder::pages(info.clone(), Metadata::default(), cfg.clone());
            for seed in 0..3 {
                let px = image(&info, seed);
                e.push_page(&px, info.row_bytes(), &info).unwrap();
            }
            e.finish_native_checked(&how_far::NoPulse).unwrap()
        });
        let c = chunks(&file);
        assert_eq!(c.last().unwrap().0, UIDX, "{form:?}");
        assert_eq!(c.iter().filter(|c| &c.0 == b"fRAM").count(), 2);
        let px = image(&info, 0);
        every_decoder(&file, &px, &format!("frames {form:?}"));
        assert!(
            RegionReader::open(SliceReader::new(&file))
                .unwrap()
                .has_unit_table()
        );
    }
}

/// Nested heads (a lossy payload's DC, alpha and exact images) carry
/// neither record 10 nor record 12: their records are the lossy units'.
#[test]
fn nested_heads_refuse_a_reservation() {
    let img = &images()[0];
    for form in [
        UnitTableForm::Hint,
        UnitTableForm::Trailing,
        UnitTableForm::Reserved(4),
        UnitTableForm::ReservedEmpty(2),
    ] {
        let file = with_unit_table(form, || img.encode(false));
        let mut r = ByteReader::new(&file);
        let hp = crate::parse::header::read_header_sized(&mut r).unwrap();
        crate::parse::chunks::walk_to_data(&mut r, |_, _| {}).unwrap();
        let mut head = ByteReader::new(&file[r.pos..]);
        assert_eq!(
            parse_head_nested(
                hp.header.info.clone(),
                &mut head,
                &crate::parse::limits::Unrestricted,
                None,
                hp.declared,
            )
            .err(),
            Some(Error::Corrupt("unit table in a nested image head")),
            "{form:?}"
        );
    }
}

/// `RegionReader::region_ranges` under every unit table form: a source that
/// serves only the planned ranges decodes the region to the full decode's
/// pixels. With a table at open (hint, filled reservation, trailing `uIDX`)
/// planning reads nothing and the region decodes in one batch.
#[test]
fn planned_region_ranges_under_every_form() {
    use alloc::rc::Rc;
    use core::cell::{Cell, RefCell};
    struct Gated<'a> {
        file: &'a [u8],
        allowed: Rc<RefCell<Option<Vec<core::ops::Range<u64>>>>>,
        batches: Rc<Cell<u32>>,
    }
    impl crate::planes::ReadAt for Gated<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.file.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            let end = offset + out.len() as u64;
            if let Some(a) = &*self.allowed.borrow()
                && !a.iter().any(|r| r.start <= offset && end <= r.end)
            {
                return Err(Error::Read);
            }
            SliceReader::new(self.file).read_exact_at(offset, out)
        }
        fn read_many_at(&mut self, reqs: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
            self.batches.set(self.batches.get() + 1);
            for (offset, out) in reqs.iter_mut() {
                self.read_exact_at(*offset, out)?;
            }
            Ok(())
        }
    }
    for img in images() {
        let (w, h) = (img.info.width, img.info.height);
        let bpp = img.info.row_bytes() / w as usize;
        let r = Rect::new(w / 4, h / 4, (w / 2).max(1), (h / 2).max(1));
        let rb = r.width() as usize * bpp;
        for form in FORMS {
            for checksum in [false, true] {
                let file = with_unit_table(form, || img.encode(checksum));
                let full = crate::decode(&file).unwrap().1;
                let want: Vec<u8> = (0..r.height() as usize)
                    .flat_map(|y| {
                        let at = ((r.y() as usize + y) * w as usize + r.x() as usize) * bpp;
                        full[at..at + rb].to_vec()
                    })
                    .collect();
                let allowed = Rc::new(RefCell::new(None));
                let batches = Rc::new(Cell::new(0));
                let src = Gated {
                    file: &file,
                    allowed: allowed.clone(),
                    batches: batches.clone(),
                };
                let mut rd = RegionReader::open(src).unwrap();
                let opened = rd.read_stats().batches;
                let plan = rd.region_ranges(r).unwrap();
                let what = alloc::format!("{} {form:?} checksum {checksum}", img.name);
                if img.multi
                    && matches!(
                        form,
                        UnitTableForm::Hint | UnitTableForm::Reserved(4) | UnitTableForm::Trailing
                    )
                {
                    assert!(rd.has_unit_table(), "{what}: table at open");
                }
                if rd.has_unit_table() {
                    assert_eq!(rd.read_stats().batches, opened, "{what}: planning read");
                }
                *allowed.borrow_mut() = Some(plan);
                batches.set(0);
                let mut got = vec![0u8; want.len()];
                rd.decode_region(r, &mut got, rb).unwrap();
                assert_eq!(got, want, "{what}");
                if rd.has_unit_table() {
                    assert_eq!(batches.get(), 1, "{what}: region batches");
                }
            }
        }
    }
}
