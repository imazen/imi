//! Helpers shared by integration tests (`mod common;`). Each test crate uses
//! a subset: fixtures and decode checks, and format walkers for the
//! container (SPEC 1.0, 1.1) and the records of a lossless payload (SPEC 2,
//! 2.7), enough to find and edit the bytes a test is about.
#![allow(dead_code)]

mod wire;
pub use wire::{chunks, header_end, header_record, varint};

pub mod checksum;
pub mod matrix;
pub mod unit_table;

use imij::decode_resources::{DecodePolicy, DecodeSession};
use std::ops::Range;

use imij::{ImageInfo, Limits, Profile, ProfileVersion};

/// Deterministic xorshift64 (seed nonzero).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// The byte range of a file's `aNIM` payload (it must have one).
pub fn anim_of(file: &[u8]) -> (usize, usize) {
    let (_, at, end) = chunks(file)
        .into_iter()
        .find(|c| &c.0 == b"aNIM")
        .expect("an aNIM chunk");
    (at + 4 + varint(&file[at + 4..]).1, end)
}

/// `file` with its `aNIM` payload replaced.
pub fn with_anim(file: &[u8], anim: &[u8]) -> Vec<u8> {
    let (a, b) = anim_of(file);
    let mut start = a - 1;
    while file[start - 1] & 0x80 != 0 {
        start -= 1;
    }
    let mut out = file[..start].to_vec();
    put_varint(&mut out, anim.len() as u64 + 1);
    out.extend_from_slice(anim);
    out.extend_from_slice(&file[b..]);
    out
}

pub const SCIENTIFIC_0_1: ProfileVersion = ProfileVersion {
    profile: Profile::Scientific,
    major: 0,
    minor: 1,
};

pub const SCIENTIFIC_0_2: ProfileVersion = ProfileVersion {
    profile: Profile::Scientific,
    major: 0,
    minor: 2,
};

/// Feature ids (SPEC 0.1) as mask bits.
pub mod feature {
    pub const SEED_BANK_1: u64 = 1;
    pub const SEED_BANK_IDS: u64 = 1 << 28;
    pub const EXACT_BLOCKS: u64 = 1 << 1;
    pub const SIGNED_SAMPLES: u64 = 1 << 2;
    pub const WIDE_SAMPLES: u64 = 1 << 3;
    pub const NEAR_LOSSLESS: u64 = 1 << 4;
    pub const FLOAT_SHIFT: u64 = 1 << 5;
    pub const MIXED_PAGES: u64 = 1 << 6;
    pub const DEQUANT_PRESETS: u64 = 1 << 7;
    pub const EXPLICIT_TABLES: u64 = 1 << 8;
    pub const RECON_BIAS: u64 = 1 << 9;
    pub const COLOR_TRANSFORM: u64 = 1 << 10;
    pub const OFFSET_MAP: u64 = 1 << 11;
    pub const SUPERBLOCKS: u64 = 1 << 12;
    pub const BILEVEL: u64 = 1 << 13;
    pub const CHANNEL_LAYOUTS: u64 = 1 << 14;
    pub const SAMPLES_32: u64 = 1 << 15;
    pub const LOSSY: u64 = 1 << 16;
    pub const TEMPLATE_PALETTES: u64 = 1 << 24;
    /// Assigned capability: named through the library's variant.
    pub const SMALL_TRANSFORMS: u64 = 1 << imij::Feature::SmallTransforms.id() as u32;
}

/// The profile versions `file` lists beyond the ones its header implies
/// (Lossless, Extended and Lossy 0.1), read through the public header.
pub fn listed_profiles(file: &[u8]) -> Vec<ProfileVersion> {
    let implied = |p: &ProfileVersion| {
        (p.major, p.minor) == (0, 1)
            && matches!(
                p.profile,
                Profile::Lossless | Profile::Extended | Profile::Lossy
            )
    };
    imij::header(file)
        .unwrap()
        .profiles
        .into_iter()
        .filter(|p| !implied(p))
        .collect()
}

/// [`listed_profiles`] without Lossless 0.3, which a file lists for the unit
/// table its payload has when it has two or more units (SPEC 2.7).
pub fn listed_beyond_table(file: &[u8]) -> Vec<ProfileVersion> {
    let table = ProfileVersion {
        profile: Profile::Lossless,
        major: 0,
        minor: 3,
    };
    listed_profiles(file)
        .into_iter()
        .filter(|p| *p != table)
        .collect()
}

/// Decodes `file` through every whole-image entry point: `decode`,
/// `decode_into`, `decode_into_threads` (1, 3, 8), `StreamDecoder`,
/// `decode_reader` and a `DecodeSession` (slice and reader). All must
/// agree; returns the pixels.
pub fn decode_all(file: &[u8], info: &ImageInfo) -> Vec<u8> {
    let (di, out) = imij::decode(file).unwrap();
    assert_eq!((di.width, di.height), (info.width, info.height));
    let rb = info.row_bytes();
    let mut o = vec![0u8; out.len()];
    imij::decode_into(file, &mut o, rb).unwrap();
    assert!(o == out, "decode_into");
    for threads in [1, 3, 8] {
        let mut o = vec![0u8; out.len()];
        imij::decode_into_threads(file, &mut o, rb, threads).unwrap();
        assert!(o == out, "decode_into_threads({threads})");
    }
    let mut d = imij::StreamDecoder::new(file).unwrap();
    let mut row = vec![0u8; rb];
    let mut y = 0;
    while d.next_row(&mut row).unwrap() {
        assert!(row[..] == out[y * rb..(y + 1) * rb], "stream row {y}");
        y += 1;
    }
    assert_eq!(y, info.height as usize);
    let mut rows = vec![0u8; out.len()];
    imij::decode_reader(std::io::Cursor::new(file), &Limits::DEFAULT, |_, y, r| {
        rows[y as usize * rb..(y as usize + 1) * rb].copy_from_slice(r);
        Ok(())
    })
    .unwrap();
    assert!(rows == out, "decode_reader");
    let mut s = DecodeSession::new(DecodePolicy::default());
    assert!(s.decode(file).unwrap().1 == out, "DecodeSession::decode");
    rows.fill(0);
    s.decode_reader(std::io::Cursor::new(file), |_, y, r| {
        rows[y as usize * rb..(y as usize + 1) * rb].copy_from_slice(r);
        Ok(())
    })
    .unwrap();
    assert!(rows == out, "DecodeSession::decode_reader");
    out
}

/// A positive retained-wire fixture also has to survive the current encoded
/// migration and reconstruct the same samples on the V4 driver. Negative V3
/// mutations keep their original grammar and expectations.
pub fn assert_current_migration_pixels(file: &[u8]) {
    let expected = imij::decode(file).expect("positive legacy fixture");
    let current = imij::upgrade_image(file, &imij::UpgradeLimits::default())
        .expect("positive retained wire must migrate");
    assert_eq!(imij::header(&current).unwrap().version, 4);
    assert_eq!(imij::decode(&current).unwrap(), expected);
}

// ---- format walkers -------------------------------------------------------

pub const SIGNATURE: &[u8] = b"IMIJ\r\n\x87\n";

/// The varint at `d[*at..]`, advancing `at` past it.
pub fn read_varint(d: &[u8], at: &mut usize) -> u64 {
    let (v, k) = varint(&d[*at..]);
    *at += k;
    v
}

/// `v` (any unsigned integer) as a varint.
pub fn varint_bytes<T: TryInto<u64>>(v: T) -> Vec<u8>
where
    T::Error: std::fmt::Debug,
{
    let mut out = Vec::new();
    put_varint(&mut out, v.try_into().unwrap());
    out
}

/// Header `present` bits (SPEC 1.0).
pub mod present {
    pub const CODING: u64 = 1;
    pub const SAMPLE: u64 = 1 << 1;
    pub const FEATURES: u64 = 1 << 2;
    pub const CHANNELS: u64 = 1 << 3;
    pub const ORIENTATION: u64 = 1 << 4;
    pub const SPEED: u64 = 1 << 5;
    pub const CICP: u64 = 1 << 6;
    pub const BITS: u64 = 1 << 7;
    pub const ALPHA: u64 = 1 << 8;
}

/// The layout of header record `h` (SPEC 1.0): `present`, the feature
/// mask's range (empty where one belongs when absent), the offset of the
/// width and of the first header record.
pub struct HeaderLayout {
    pub present: u64,
    pub features: Range<usize>,
    pub dims: usize,
    pub records: usize,
}

pub fn header_layout(h: &[u8]) -> HeaderLayout {
    assert!(matches!(h[0], 3 | 4), "header version");
    let (p, k) = varint(&h[1..]);
    let mut at = 1 + k;
    let features = if p & present::FEATURES != 0 {
        let k = varint(&h[at..]).1;
        at..at + k
    } else {
        at..at
    };
    at = features.end;
    let mut nch = 3;
    for (bit, bytes) in [
        (present::CODING, 1),
        (present::SAMPLE, 1),
        (present::CHANNELS, 1),
        (present::ORIENTATION, 1),
        (present::SPEED, 1),
        (present::CICP, 4),
        (present::BITS, 0),
        (present::ALPHA, 1),
    ] {
        if p & bit == 0 {
            continue;
        }
        if bit == present::CHANNELS {
            nch = match h[at] {
                1..=4 => h[at] as usize,
                5 | 7 => 4,
                6 => 3,
                _ => panic!("unknown test channel layout"),
            };
        }
        at += if bit == present::BITS { nch } else { bytes };
    }
    let dims = at;
    at += varint(&h[at..]).1; // width
    at += varint(&h[at..]).1; // height
    HeaderLayout {
        present: p,
        features,
        dims,
        records: at,
    }
}

/// Where the header records start in header record `h`: after the fields
/// its `present` mask selects (SPEC 1.0).
fn extensions_at(h: &[u8]) -> usize {
    header_layout(h).records
}

/// The feature mask a file declares (SPEC 0.1), 0 without one.
pub fn feature_mask(file: &[u8]) -> u64 {
    let h = &file[header_record(file)];
    let l = header_layout(h);
    if l.features.is_empty() {
        0
    } else {
        varint(&h[l.features]).0
    }
}

/// The lowest feature bit no feature is assigned to (a newer decoder's), from
/// the public mask parser, so tests of unknown features do not hard-code a
/// number that the next feature takes.
#[allow(dead_code)]
pub fn unassigned_feature() -> u64 {
    (0..64)
        .map(|k| 1u64 << k)
        .find(|&b| imij::Features::from_bits(b).is_none())
        .expect("a free feature bit")
}

/// `file` with its feature mask replaced (`None`: the field removed);
/// every other byte is kept.
pub fn with_feature_mask(file: &[u8], mask: Option<u64>) -> Vec<u8> {
    let bytes = mask.map(|m| {
        let mut b = Vec::new();
        put_varint(&mut b, m);
        b
    });
    with_feature_mask_bytes(file, bytes.as_deref())
}

/// [`with_feature_mask`] with the mask's bytes as given (SPEC 1.0: an
/// LEB128 bit string of any length).
pub fn with_feature_mask_bytes(file: &[u8], mask: Option<&[u8]>) -> Vec<u8> {
    let r = header_record(file);
    let h = &file[r.clone()];
    let l = header_layout(h);
    let mut head = vec![h[0]];
    let p = match mask {
        Some(_) => l.present | present::FEATURES,
        None => l.present & !present::FEATURES,
    };
    put_varint(&mut head, p);
    if let Some(m) = mask {
        head.extend_from_slice(m);
    }
    head.extend_from_slice(&h[l.features.end..]);
    let mut out = file[..8].to_vec();
    put_varint(&mut out, head.len() as u64);
    out.extend_from_slice(&head);
    out.extend_from_slice(&file[r.end..]);
    out
}

/// `file` with its header's width and height rewritten (and nothing else):
/// for tests of what a reader allocates from the header alone.
pub fn with_dimensions(file: &[u8], width: u64, height: u64) -> Vec<u8> {
    let h = header_record(file);
    let rec = &file[h.clone()];
    let l = header_layout(rec);
    let mut head = rec[..l.dims].to_vec();
    put_varint(&mut head, width);
    put_varint(&mut head, height);
    head.extend_from_slice(&rec[l.records..]);
    let mut out = file[..8].to_vec();
    put_varint(&mut out, head.len() as u64);
    out.extend_from_slice(&head);
    out.extend_from_slice(&file[h.end..]);
    out
}

/// Header extension records: (id, payload range in `file`), in file order.
pub fn extension_ranges(file: &[u8]) -> Vec<(u64, Range<usize>)> {
    let h = header_record(file);
    let mut at = h.start + extensions_at(&file[h.clone()]);
    let mut v = Vec::new();
    while at < h.end {
        let (id, a) = varint(&file[at..]);
        let (len, b) = varint(&file[at + a..]);
        let start = at + a + b;
        v.push((id, start..start + len as usize));
        at = start + len as usize;
    }
    v
}

/// Header extension records: (id, payload), in file order.
pub fn extensions(file: &[u8]) -> Vec<(u64, Vec<u8>)> {
    extension_ranges(file)
        .into_iter()
        .map(|(id, r)| (id, file[r].to_vec()))
        .collect()
}

/// `file` with its header extension records replaced by `recs`.
pub fn with_extensions(file: &[u8], recs: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let h = header_record(file);
    let mut head = file[h.start..h.start + extensions_at(&file[h.clone()])].to_vec();
    for (id, p) in recs {
        put_varint(&mut head, *id);
        put_varint(&mut head, p.len() as u64);
        head.extend_from_slice(p);
    }
    let mut out = file[..8].to_vec();
    put_varint(&mut out, head.len() as u64);
    out.extend_from_slice(&head);
    out.extend_from_slice(&file[h.end..]);
    out
}

/// The header without the extension records `ids`: what two files share
/// when they differ only in those records.
pub fn header_without(file: &[u8], ids: &[u64]) -> Vec<u8> {
    let recs: Vec<_> = extensions(file)
        .into_iter()
        .filter(|e| !ids.contains(&e.0))
        .collect();
    let f = with_extensions(file, &recs);
    f[header_record(&f)].to_vec()
}

/// Chunks after the header: (tag, chunk start, chunk end). An open chunk
/// (length 0, SPEC 1.1) is taken to run to the end of the file.
/// Chunks as (tag, payload).
pub type Chunks = Vec<([u8; 4], Vec<u8>)>;

/// A file's header record and its chunks (an open chunk's payload runs to
/// the end of the file).
pub fn parts(file: &[u8]) -> (Vec<u8>, Chunks) {
    let chunks = chunks(file)
        .into_iter()
        .map(|(tag, at, end)| (tag, file[at + 4 + varint(&file[at + 4..]).1..end].to_vec()))
        .collect();
    (file[header_record(file)].to_vec(), chunks)
}

/// The file of header record `header` and `chunks`, each closed (with its
/// length) except `DATA` when `open_data`.
pub fn join(header: &[u8], chunks: &[([u8; 4], Vec<u8>)], open_data: bool) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    put_varint(&mut out, header.len() as u64);
    out.extend_from_slice(header);
    for (tag, payload) in chunks {
        if open_data && tag == b"DATA" {
            out.extend_from_slice(tag);
            out.push(0);
            out.extend_from_slice(payload);
        } else {
            put_chunk(&mut out, tag, payload);
        }
    }
    out
}

/// Appends a closed chunk (tag, payload length + 1, payload).
pub fn put_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(tag);
    put_varint(out, payload.len() as u64 + 1);
    out.extend_from_slice(payload);
}

/// The `DATA` payload's range in `file`.
pub fn data_payload(file: &[u8]) -> Range<usize> {
    let (_, at, end) = chunks(file)
        .into_iter()
        .find(|c| &c.0 == b"DATA")
        .expect("a DATA chunk");
    at + 4 + varint(&file[at + 4..]).1..end
}

/// The parts of a `DATA` payload (SPEC 2-2.8), as offsets into it.
#[derive(Clone, Debug)]
pub struct Payload {
    /// Offset of the head's `present` varint (lossless: 0; lossy: after the
    /// stripe height and the column group width) and its value.
    pub present_at: usize,
    pub present: u64,
    /// The `present` bit that says a head record list follows.
    pub records_bit: u64,
    /// Lossless: the entropy models; lossy: the AC tables.
    pub models: Range<usize>,
    /// The head record list's length varint and the list itself.
    pub list: Option<(usize, Range<usize>)>,
    /// Each head record: (id, payload range).
    pub head_records: Vec<(u64, Range<usize>)>,
    /// Lossy: the nested heads (DC, alpha, exact), after their lengths.
    pub nested: Vec<Range<usize>>,
    /// The end of the head: the first unit record.
    pub head_end: usize,
    /// Every unit record, side records included: (id, whole record,
    /// payload).
    pub units: Vec<(u64, Range<usize>, Range<usize>)>,
}

impl Payload {
    /// The payload of head record `id`.
    pub fn head_record(&self, id: u64) -> Option<Range<usize>> {
        self.head_records
            .iter()
            .find(|r| r.0 == id)
            .map(|r| r.1.clone())
    }
    /// The unit table hint's payload (head record 10).
    pub fn table(&self) -> Option<Range<usize>> {
        self.head_record(H_UNIT_TABLE)
    }
    /// The main (odd-id) unit records' payloads, in record order.
    pub fn records(&self) -> Vec<Range<usize>> {
        self.units
            .iter()
            .filter(|u| u.0 & 1 == 1)
            .map(|u| u.2.clone())
            .collect()
    }
    /// The main unit records, whole, in record order.
    pub fn framed(&self) -> Vec<Range<usize>> {
        self.units
            .iter()
            .filter(|u| u.0 & 1 == 1)
            .map(|u| u.1.clone())
            .collect()
    }
}

/// Lossy head record id of superblocks (SPEC 2.9).
pub const H_SUPERBLOCKS: u64 = 21;
pub const H_UNIT_TABLE: u64 = 10;
pub const H_UNIT_RESERVE: u64 = 12;
pub const H_PARTITIONS: u64 = 23;

/// Current lossy channel-layout head record (SPEC registry).
pub const H_LAYOUTS: u64 = 27;

/// The partition maps of a lossy file's units (SPEC 2.9): per AC record
/// (id 5) the map's length, 0 where the unit merges nothing. Empty without
/// head record 21.
pub fn superblock_map_lengths(file: &[u8], info: &ImageInfo) -> Vec<usize> {
    let d = &file[data_payload(file)];
    let p = payload(d, info, true);
    if p.head_record(H_SUPERBLOCKS).is_none() {
        return Vec::new();
    }
    p.units
        .iter()
        .filter(|u| u.0 == 55)
        .map(|u| {
            let mut at = u.2.start;
            read_varint(d, &mut at) as usize
        })
        .collect()
}

/// Units of a lossy file whose partition map merges blocks (a map that
/// merges nothing is `Corrupt`, SPEC 2.9, so a non-empty map is a merge).
pub fn superblock_units(file: &[u8], info: &ImageInfo) -> usize {
    superblock_map_lengths(file, info)
        .iter()
        .filter(|&&n| n > 0)
        .count()
}

/// SPEC 2.9.2's binary range coder (LZMA's), encoder side.
struct Rc {
    low: u64,
    range: u32,
    cache: u8,
    pending: u64,
    out: Vec<u8>,
}

impl Rc {
    fn new() -> Self {
        Rc {
            low: 0,
            range: u32::MAX,
            cache: 0,
            pending: 1,
            out: Vec::new(),
        }
    }
    fn shift(&mut self) {
        if (self.low as u32) < 0xff00_0000 || self.low >> 32 != 0 {
            let carry = (self.low >> 32) as u8;
            let mut b = self.cache;
            loop {
                self.out.push(b.wrapping_add(carry));
                b = 0xff;
                self.pending -= 1;
                if self.pending == 0 {
                    break;
                }
            }
            self.cache = (self.low >> 24) as u8;
        }
        self.pending += 1;
        self.low = (self.low & 0x00ff_ffff) << 8;
    }
    fn bit(&mut self, p: &mut u16, bit: bool) {
        let bound = (self.range >> 12) * *p as u32;
        if bit {
            self.low += bound as u64;
            self.range -= bound;
            *p -= *p >> 5;
        } else {
            self.range = bound;
            *p += (4096 - *p) >> 5;
        }
        while self.range < 1 << 24 {
            self.range <<= 8;
            self.shift();
        }
    }
    /// The code value of the final interval with the most trailing zero
    /// bits; then five flush bytes, the leading carry byte and the trailing
    /// zero bytes dropped.
    fn finish(mut self) -> Vec<u8> {
        let hi = self.low + self.range as u64;
        for k in (0..=32).rev() {
            let mask = (1u64 << k) - 1;
            let v = (self.low + mask) & !mask;
            if v < hi {
                self.low = v;
                break;
            }
        }
        for _ in 0..5 {
            self.shift();
        }
        let mut out = self.out;
        out.remove(0);
        while out.last() == Some(&0) {
            out.pop();
        }
        out
    }
}

/// The partition symbols of SPEC 2.9.2 (`sym(r, x)` at the group whose
/// top-left parent is (r, x)) into `rc`; whether any is not 0.
fn code_symbols(rc: &mut Rc, rows: usize, cols: usize, sym: &dyn Fn(usize, usize) -> u8) -> bool {
    let (mut any, mut tree) = ([2048u16; 9], [[2048u16; 9]; 8]);
    let gc = cols.div_ceil(2);
    let mut grid = vec![None::<u8>; rows.div_ceil(2) * gc];
    let class = |s: Option<u8>| match s {
        None | Some(0) => 0,
        Some(1) => 1,
        _ => 2,
    };
    let mut coded = false;
    for r in (0..rows).step_by(2) {
        for x in (0..cols).step_by(2) {
            if r + 1 >= rows && x + 1 >= cols {
                continue; // a lone corner parent carries no symbol
            }
            let (gr, g) = (r / 2, x / 2);
            let left = g.checked_sub(1).and_then(|l| grid[gr * gc + l]);
            let up = gr.checked_sub(1).and_then(|u| grid[u * gc + g]);
            let ctx = class(left) * 3 + class(up);
            let s = sym(r, x);
            rc.bit(&mut any[ctx], s != 0);
            if s != 0 {
                coded = true;
                let mut node = 1;
                for b in (0..3).rev() {
                    let bit = (s - 1) >> b & 1 != 0;
                    rc.bit(&mut tree[node][ctx], bit);
                    node = node * 2 + usize::from(bit);
                }
            }
            grid[gr * gc + g] = Some(s);
        }
    }
    coded
}

/// A partition map (SPEC 2.9) of a `rows` x `cols` unit with symbol
/// `sym(r, x)` at the group whose top-left parent is (r, x): 0 no merge,
/// 1..=7 the partitions, 8 tree value 7. Symbols are coded as given, those
/// that need an absent parent included, so tests can forge maps the
/// crate's writer never makes. The coder follows SPEC 2.9 (LZMA's binary
/// range coder and its flush). Empty when every symbol is 0.
pub fn superblock_map(rows: usize, cols: usize, sym: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let mut rc = Rc::new();
    if !code_symbols(&mut rc, rows, cols, &sym) {
        return Vec::new();
    }
    rc.finish()
}

/// A joint partition map (SPEC 2.11.1) of a `rows` x `cols` unit: the unit
/// header, the symbols of [`superblock_map`] when one is not 0, then when a
/// coded flag is 1, for every parent (r, x) in raster
/// order with `flags(r, x) = Some(f)` a split flag (bit c of `f`) for each
/// plane c with AC (`ac(r, x)`, bit c), contexts as SPEC 2.11.1 (a parent
/// with `None` counts as merged or bypassed; a plane without AC counts as
/// 0). Coded as given, whatever the symbols merge, so tests can forge flags
/// of merged parents. Written from the SPEC, independently of the crate's
/// writer; never empty.
pub fn joint_map(
    rows: usize,
    cols: usize,
    sym: impl Fn(usize, usize) -> u8,
    flags: impl Fn(usize, usize) -> Option<u8>,
    ac: impl Fn(usize, usize) -> u8,
) -> Vec<u8> {
    joint_map_with_header(rows, cols, sym, flags, ac, None)
}

/// [`joint_map`] with the unit header given (`Some((merges, splits))`)
/// instead of derived from the content (any symbol not 0, any coded flag
/// 1), for forging headers that disagree with their sections.
pub fn joint_map_with_header(
    rows: usize,
    cols: usize,
    sym: impl Fn(usize, usize) -> u8,
    flags: impl Fn(usize, usize) -> Option<u8>,
    ac: impl Fn(usize, usize) -> u8,
    header: Option<(bool, bool)>,
) -> Vec<u8> {
    let mut rc = Rc::new();
    // The unit header (probability 1/2 each), then the sections it
    // announces.
    let (merges, split) = header.unwrap_or_else(|| {
        let m = (0..rows).step_by(2).any(|r| {
            (0..cols)
                .step_by(2)
                .any(|x| !(r + 1 >= rows && x + 1 >= cols) && sym(r, x) != 0)
        });
        let s =
            (0..rows).any(|r| (0..cols).any(|x| flags(r, x).is_some_and(|f| f & ac(r, x) != 0)));
        (m, s)
    });
    rc.bit(&mut 2048, merges);
    rc.bit(&mut 2048, split);
    if merges {
        code_symbols(&mut rc, rows, cols, &sym);
    }
    if !split {
        return rc.finish();
    }
    let mut p = [2048u16; 54];
    let class = |r: Option<usize>, x: Option<usize>, c: usize| match (r, x) {
        (Some(r), Some(x)) if r < rows && x < cols => {
            flags(r, x).map_or(2, |f| ((f & ac(r, x)) >> c & 1) as usize)
        }
        _ => 2,
    };
    for r in 0..rows {
        for x in 0..cols {
            let Some(f) = flags(r, x) else { continue };
            let a = ac(r, x);
            for c in 0..3 {
                if a >> c & 1 == 0 {
                    continue;
                }
                let left = class(Some(r), x.checked_sub(1), c);
                let up = class(r.checked_sub(1), Some(x), c);
                let luma = if c == 0 { 0 } else { (f & a & 1) as usize };
                rc.bit(
                    &mut p[((3 * c + left) * 3 + up) * 2 + luma],
                    f >> c & 1 != 0,
                );
            }
        }
    }
    rc.finish()
}

/// Reads a record list (`id varint, len varint, payload`) filling `d[at..end]`.
fn record_list(d: &[u8], mut at: usize, end: usize) -> Vec<(u64, Range<usize>, Range<usize>)> {
    let mut v = Vec::new();
    while at < end {
        let start = at;
        let id = read_varint(d, &mut at);
        let n = read_varint(d, &mut at) as usize;
        v.push((id, start..at + n, at..at + n));
        at += n;
    }
    assert_eq!(at, end, "records fill their list");
    v
}

/// Walks the payload `d` of a file with image `info` and coding `lossy`.
pub fn payload(d: &[u8], info: &ImageInfo, lossy: bool) -> Payload {
    let mut p = payload_head(d, info, lossy);
    p.units = record_list(d, p.head_end, d.len());
    p
}

/// [`payload`] of the head only (no unit records walked): for payloads
/// whose records a test cut or forged.
pub fn payload_head(d: &[u8], info: &ImageInfo, lossy: bool) -> Payload {
    let mut at = 0;
    let records_bit;
    let present_at;
    let present;
    let models;
    if lossy {
        at += 1; // stripe height
        read_varint(d, &mut at); // column group width
        present_at = at;
        present = read_varint(d, &mut at);
        records_bit = 16;
        at += 4; // quantizer scale
        for (bit, n) in [(0, 1), (1, 1), (2, 1), (3, 3), (6, 4), (7, 12), (8, 12)] {
            if present >> bit & 1 != 0 {
                at += n;
            }
        }
        if present & (1 << 9) != 0 {
            read_varint(d, &mut at); // bank ID
            read_varint(d, &mut at); // seed scope
        }
        let n = read_varint(d, &mut at) as usize;
        models = at..at + n;
        at += n;
    } else {
        present_at = 0;
        present = read_varint(d, &mut at);
        assert_eq!(present & 4, 0, "lab seed-bank heads are not walked here");
        records_bit = 2;
        at += 1; // lanes
        read_varint(d, &mut at); // stripe rows
        read_varint(d, &mut at); // column group width
        let words = if info.sample.bytes() == 8 { 2 } else { 1 };
        let value = if info.sample.bytes() >= 4 { 4 } else { 2 };
        if present & 8 == 0 {
            for _ in 0..info.channels.count() * words {
                // Map 9 (SPEC 2.1): its table's varints follow the value.
                let (mode, entries) = (d[at], u16::from_le_bytes([d[at + 2], d[at + 3]]));
                at += 2 + value;
                if mode == 9 {
                    for _ in 0..=entries {
                        read_varint(d, &mut at);
                    }
                }
            }
            let n = read_varint(d, &mut at) as usize;
            models = at..at + n;
            at += n;
        } else {
            // Quick heads omit maps and entropy models (SPEC 2.13).
            models = at..at;
        }
    }
    let mut list = None;
    let mut head_records = Vec::new();
    if present & records_bit != 0 {
        let len_at = at;
        let n = read_varint(d, &mut at) as usize;
        head_records = record_list(d, at, at + n)
            .into_iter()
            .map(|(id, _, p)| (id, p))
            .collect();
        list = Some((len_at, at..at + n));
        at += n;
    }
    let mut nested = Vec::new();
    if lossy {
        let exact = head_records.iter().any(|r| r.0 == 5);
        for _ in 0..1 + usize::from(info.channels.has_alpha()) + usize::from(exact) {
            let n = read_varint(d, &mut at) as usize;
            nested.push(at..at + n);
            at += n;
        }
    }
    let head_end = at;
    Payload {
        present_at,
        present,
        records_bit,
        models,
        list,
        head_records,
        nested,
        head_end,
        units: Vec::new(),
    }
}

/// `d` (a payload [`payload`] walked as `p`) with head record `id` set to
/// `rec` (None: removed), the list kept in id order; `present` says whether
/// a list follows, and the list goes when it is left empty.
pub fn with_head_record(d: &[u8], p: &Payload, id: u64, rec: Option<&[u8]>) -> Vec<u8> {
    let mut recs: Vec<(u64, Vec<u8>)> = p
        .head_records
        .iter()
        .filter(|r| r.0 != id)
        .map(|r| (r.0, d[r.1.clone()].to_vec()))
        .collect();
    if let Some(r) = rec {
        let at = recs.partition_point(|e| e.0 < id);
        recs.insert(at, (id, r.to_vec()));
    }
    let mut list = Vec::new();
    for (id, r) in &recs {
        put_varint(&mut list, *id);
        put_varint(&mut list, r.len() as u64);
        list.extend_from_slice(r);
    }
    let mut out = d[..p.present_at].to_vec();
    let present = if list.is_empty() {
        p.present & !p.records_bit
    } else {
        p.present | p.records_bit
    };
    put_varint(&mut out, present);
    let mut at = p.present_at;
    read_varint(d, &mut at);
    // The list sits right after the models (lossless) or tables (lossy).
    out.extend_from_slice(&d[at..p.models.end]);
    if !list.is_empty() {
        put_varint(&mut out, list.len() as u64);
        out.extend_from_slice(&list);
    }
    let rest = p.list.as_ref().map_or(p.models.end, |l| l.1.end);
    out.extend_from_slice(&d[rest..]);
    out
}

/// `d` (a payload [`payload`] walked as `p`) without its unit table hint:
/// readers then find the records by their own lengths.
pub fn without_hint(d: &[u8], p: &Payload) -> Vec<u8> {
    assert!(p.table().is_some(), "a unit table hint");
    with_head_record(d, p, H_UNIT_TABLE, None)
}

/// `file` with `extra` bytes appended to the payload of its `i`-th main
/// unit record (its length and its unit table hint entry grown to match):
/// bytes the record's decode never reads (SPEC 2.7's slack).
pub fn with_record_slack(file: &[u8], i: usize, extra: &[u8]) -> Vec<u8> {
    with_main_record(file, i, |pay| [pay, extra].concat())
}

/// `file` (lossy) with the partition map of its `u`-th AC record (id 5,
/// SPEC 2.9) replaced by `map` (an empty map: no merge).
pub fn with_superblock_map(file: &[u8], u: usize, map: &[u8]) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let p = payload(&file[data_payload(file)], &h.info, true);
    let ac = p.units.iter().filter(|r| r.0 == 55).nth(u).expect("unit u");
    let i = p
        .units
        .iter()
        .filter(|r| r.0 & 1 == 1)
        .position(|r| r.1 == ac.1)
        .unwrap();
    with_main_record(file, i, |pay| {
        let mut at = 0;
        let n = read_varint(pay, &mut at) as usize;
        let mut out = Vec::new();
        put_varint(&mut out, map.len() as u64);
        out.extend_from_slice(map);
        out.extend_from_slice(&pay[at + n..]);
        out
    })
}

/// `file` (lossy, without partition maps) with `map` and its length varint
/// put at the start of AC record `u` (unit order).
pub fn with_inserted_map(file: &[u8], u: usize, map: &[u8]) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let p = payload(&file[data_payload(file)], &h.info, true);
    let ac = p.units.iter().filter(|r| r.0 == 55).nth(u).expect("unit u");
    let i = p
        .units
        .iter()
        .filter(|r| r.0 & 1 == 1)
        .position(|r| r.1 == ac.1)
        .unwrap();
    with_main_record(file, i, |pay| {
        let mut out = Vec::new();
        put_varint(&mut out, map.len() as u64);
        out.extend_from_slice(map);
        out.extend_from_slice(pay);
        out
    })
}

/// The units of a lossy file (SPEC 2.7, 2.9.1): per AC record, in unit
/// order, its block rows and block columns.
pub fn lossy_units(file: &[u8]) -> Vec<(usize, usize)> {
    let h = imij::header(file).unwrap();
    let d = &file[data_payload(file)];
    let sb = d[0] as usize;
    let mut at = 1;
    let gcols = read_varint(d, &mut at) as usize;
    let (w, ht) = (h.info.width as usize, h.info.height as usize);
    let (bw, bh) = (w.div_ceil(8), ht.div_ceil(8));
    let groups: Vec<usize> = if gcols == 0 {
        vec![bw]
    } else {
        (0..w)
            .step_by(gcols)
            .map(|x0| gcols.min(w - x0).div_ceil(8))
            .collect()
    };
    let mut out = Vec::new();
    for by0 in (0..bh).step_by(sb) {
        let rows = sb.min(bh - by0);
        for &c in &groups {
            out.push((rows, c));
        }
    }
    out
}

/// `file` with the payload of its `i`-th main unit record replaced by
/// `edit` of it (its length and its unit table hint entry rewritten).
fn with_main_record(file: &[u8], i: usize, edit: impl FnOnce(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let range = data_payload(file);
    let d = &file[range.clone()];
    let p = payload(d, &h.info, h.coding == imij::Coding::Lossy);
    let main: Vec<usize> = (0..p.units.len())
        .filter(|&u| p.units[u].0 & 1 == 1)
        .collect();
    let (id, whole, pay) = p.units[main[i]].clone();
    let new = edit(&d[pay]);
    let mut grown = Vec::new();
    put_varint(&mut grown, id);
    put_varint(&mut grown, new.len() as u64);
    grown.extend_from_slice(&new);
    let mut head = d[..p.head_end].to_vec();
    if let Some(t) = p.table() {
        let mut entries = Vec::new();
        let mut at = t.start;
        while at < t.end {
            entries.push(read_varint(d, &mut at));
        }
        entries[i] = entries[i] - whole.len() as u64 + grown.len() as u64;
        let mut hint = Vec::new();
        for e in entries {
            put_varint(&mut hint, e);
        }
        head = with_head_record(
            &head,
            &payload_head(&head, &h.info, h.coding == imij::Coding::Lossy),
            H_UNIT_TABLE,
            Some(&hint),
        );
    }
    let mut body = head;
    body.extend_from_slice(&d[p.head_end..whole.start]);
    body.extend_from_slice(&grown);
    body.extend_from_slice(&d[whole.end..]);
    let (_, at, _) = chunks(file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let mut out = file[..at + 4].to_vec();
    put_varint(&mut out, body.len() as u64 + 1);
    out.extend_from_slice(&body);
    out.extend_from_slice(&file[range.end..]);
    out
}

/// `file` (lossy) with lossy head record `id` set to `rec` (None: removed).
pub fn with_lossy_head_record(file: &[u8], id: u64, rec: Option<&[u8]>) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let range = data_payload(file);
    let d = &file[range.clone()];
    let p = payload_head(d, &h.info, true);
    let body = with_head_record(d, &p, id, rec);
    let (_, at, _) = chunks(file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let mut out = file[..at + 4].to_vec();
    put_varint(&mut out, body.len() as u64 + 1);
    out.extend_from_slice(&body);
    out.extend_from_slice(&file[range.end..]);
    out
}

/// The parts of a lossless `DATA` payload (SPEC 2, 2.7), as offsets
/// into it.
pub struct LosslessPayload {
    pub payload: Payload,
    /// The unit table hint's payload, when the head has one.
    pub table: Option<Range<usize>>,
    /// Each unit's record payload, in unit order (stripe-major, then column
    /// group).
    pub records: Vec<Range<usize>>,
}

/// Walks lossless payload `d` of an image `info`: one stripe record per
/// unit, with or without the unit table hint.
pub fn lossless_payload(d: &[u8], info: &ImageInfo) -> LosslessPayload {
    let p = payload(d, info, false);
    assert!(
        p.units.iter().all(|u| u.0 == 59 || u.0 == 78),
        "stripe records"
    );
    LosslessPayload {
        table: p.table(),
        records: p.records(),
        payload: p,
    }
}

// ---- an independent lossless writer for forged models (SPEC 2, 3) ----------

/// An LSB-first bit writer (SPEC 0: bit streams are LSB-first).
#[derive(Default)]
pub struct Bits {
    pub out: Vec<u8>,
    n: u64,
}

impl Bits {
    pub fn put(&mut self, v: u64, bits: u32) {
        for i in 0..bits {
            if self.n.is_multiple_of(8) {
                self.out.push(0);
            }
            *self.out.last_mut().unwrap() |= ((v >> i & 1) as u8) << (self.n % 8);
            self.n += 1;
        }
    }

    /// `gamma(v)` of SPEC 3.3: `z` zero bits, a one bit, the low `z` bits.
    pub fn gamma(&mut self, v: u32) {
        let z = 31 - v.leading_zeros();
        self.put(0, z);
        self.put(1, 1);
        self.put(u64::from(v) & ((1 << z) - 1), z);
    }
}

/// One slot model (SPEC 3.3) with every context class in context 0 and one
/// table of `weights` at precision 8 (each weight below 256).
pub fn put_model(b: &mut Bits, k: u32, m: u32, depth: u32, weights: &[u32]) {
    put_model_head(b, k, m, false, (depth > 16).then_some(0));
    put_table(b, weights);
}

/// A model's fields before its tables (SPEC 3.3), written as given:
/// `ctx_shift` when `Some`, and a ctx_map with every class in context 0.
pub fn put_model_head(b: &mut Bits, k: u32, m: u32, cross: bool, ctx_shift: Option<u32>) {
    b.put(k.into(), 5);
    b.put(m.into(), 3);
    b.put(cross.into(), 1);
    if let Some(sh) = ctx_shift {
        b.put(sh.into(), 5);
    }
    b.put(0, 32);
}

/// One table of `weights` (SPEC 3.3) at precision 8: `last`, `prec - 1`,
/// then each exponent's gamma code and mantissa. Weights may have up to 8
/// significant bits (any exponent up to 32).
pub fn put_table(b: &mut Bits, weights: &[u32]) {
    let last = weights.iter().rposition(|&w| w > 0).unwrap_or(0);
    b.put(last as u64, 8);
    b.put(7, 3); // precision 8
    let mut prev = 0i64;
    for &w in &weights[..=last] {
        let e = 32 - w.leading_zeros();
        let d = i64::from(e) - prev;
        let z = if d >= 0 { 2 * d } else { -2 * d - 1 };
        b.gamma(z as u32 + 1);
        if e > 1 {
            let nb = (e - 1).min(7);
            assert_eq!(w & ((1 << (e - 1 - nb)) - 1), 0, "8 significant bits");
            b.put(u64::from(w >> (e - 1 - nb)) & ((1 << nb) - 1), nb);
        }
        prev = e.into();
    }
}

/// SPEC 3.3's normalization of `weights` to frequencies summing to 1024.
pub fn normalize(weights: &[u32]) -> Vec<u32> {
    let total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
    let mut f: Vec<u32> = weights
        .iter()
        .map(|&w| match w {
            0 => 0,
            w => ((u64::from(w) * 1024 / total) as u32).max(1),
        })
        .collect();
    let mut order: Vec<usize> = (0..f.len()).filter(|&i| weights[i] > 0).collect();
    order.sort_by(|&a, &b| weights[b].cmp(&weights[a]).then(a.cmp(&b)));
    let mut sum: u32 = f.iter().sum();
    let mut i = 0;
    while sum < 1024 {
        f[order[i % order.len()]] += 1;
        sum += 1;
        i += 1;
    }
    while sum > 1024 {
        let s = order[i % order.len()];
        if f[s] > 1 {
            f[s] -= 1;
            sum -= 1;
        }
        i += 1;
    }
    f
}

/// The stream of one active lane (SPEC 3, 3.1) that decodes `syms`, each
/// (symbol, extra bits, extra bit count) under frequencies `freq`: the
/// initial state, then the words in decode order.
pub fn one_lane_stream(freq: &[u32], syms: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut cum = vec![0u32; freq.len() + 1];
    for (i, &f) in freq.iter().enumerate() {
        cum[i + 1] = cum[i] + f;
    }
    let mut x: u32 = 1 << 16;
    let mut words: Vec<u16> = Vec::new();
    let push_bits = |x: &mut u32, words: &mut Vec<u16>, v: u32, n: u32| {
        if u64::from(*x) >= ((1u64 << 16) >> n) << 16 {
            words.push(*x as u16);
            *x >>= 16;
        }
        *x = (*x << n) | v;
    };
    for &(s, e, nb) in syms.iter().rev() {
        if nb > 16 {
            push_bits(&mut x, &mut words, e >> 16, nb - 16);
        }
        if nb > 0 {
            let n = nb.min(16);
            push_bits(&mut x, &mut words, e & ((1 << n) - 1), n);
        }
        let (f, c) = (freq[s as usize], cum[s as usize]);
        assert!(f > 0, "symbol {s} has weight");
        if u64::from(x) >= u64::from(f) << 22 {
            words.push(x as u16);
            x >>= 16;
        }
        x = ((x / f) << 10) + x % f + c;
    }
    let mut out = x.to_le_bytes().to_vec();
    for w in words.iter().rev() {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out
}

/// A Gray u8 lossless file written from SPEC 1.0, 2 and 3 alone: one unit
/// (stripe height `height`, no column groups), one lane, an Lsb map of 8
/// bits, every row predicted from W, and slot 0's model (`k`, `m`,
/// `weights`) and symbols (`syms`, as [`one_lane_stream`] takes them)
/// as given, so tests can forge what the crate's writer never writes.
pub fn one_lane_gray8(
    width: u32,
    height: u32,
    k: u32,
    m: u32,
    weights: &[u32],
    syms: &[(u32, u32, u32)],
) -> Vec<u8> {
    let mut models = Bits::default();
    put_model(&mut models, k, m, 8, weights);
    let stream = one_lane_stream(&normalize(weights), syms);
    gray8_file(
        width,
        height,
        0,
        &models.out,
        &gray8_stripe(height, &stream),
    )
}

/// A one-slot stripe (SPEC 2.4): decorrelation 0, predictor W on every row,
/// the slot's `stream`.
pub fn gray8_stripe(height: u32, stream: &[u8]) -> Vec<u8> {
    let mut stripe = vec![0u8]; // decorrelation
    stripe.resize(1 + (height as usize).div_ceil(2), 0); // predictor W
    put_varint(&mut stripe, stream.len() as u64);
    stripe.extend_from_slice(stream);
    stripe
}

/// A Gray u8 lossless file (SPEC 1.0, 2): header version 3 (present =
/// channels, Gray), then `DATA` with head present bits `present`, one lane,
/// one unit of `height` rows, an Lsb map of 8 bits, `models` as the models
/// bytes, and one stripe record of `stripe`.
pub fn gray8_file(width: u32, height: u32, present: u64, models: &[u8], stripe: &[u8]) -> Vec<u8> {
    let mut header = vec![3, 1 << 3, 1];
    put_varint(&mut header, width.into());
    put_varint(&mut header, height.into());
    let mut d = Vec::new();
    put_varint(&mut d, present);
    d.push(1); // lanes
    put_varint(&mut d, height.into()); // stripe rows
    put_varint(&mut d, 0); // column groups
    d.extend_from_slice(&[0, 8, 0, 0]); // Lsb map, 8 bits, value 0
    put_varint(&mut d, models.len() as u64);
    d.extend_from_slice(models);
    put_varint(&mut d, 59); // a lossless stripe record
    put_varint(&mut d, stripe.len() as u64);
    d.extend_from_slice(stripe);
    join(&header, &[(*b"DATA", d)], false)
}

/// Value tokens of the k = 4, m = 0 scheme of an 8-bit slot (SPEC 3.2):
/// 0..=15 direct, then one per bit length 4..=7. Its 36 run tokens follow:
/// 16 direct run lengths, then one per bit length 4..=23 of `len - 1`.
pub const K4_VALUE_TOKENS: u32 = 20;
pub const K4_ALPHABET: usize = 56;

/// The k = 4, m = 0 value token of residual `u` (below 16).
pub fn value_token(u: u32) -> (u32, u32, u32) {
    assert!(u < 16);
    (u, 0, 0)
}

/// The k = 4, m = 0 run token of a run of `len` positions: (symbol, extra
/// bits, extra bit count).
pub fn run_token(len: u32) -> (u32, u32, u32) {
    let u = len - 1;
    if u < 16 {
        (K4_VALUE_TOKENS + u, 0, 0)
    } else {
        let msb = 31 - u.leading_zeros();
        (K4_VALUE_TOKENS + 16 + msb - 4, u - (1 << msb), msb)
    }
}

/// [`one_lane_gray8`] with the k = 4, m = 0 scheme, weighting exactly the
/// symbols `syms` uses.
pub fn one_lane_k4(width: u32, height: u32, syms: &[(u32, u32, u32)]) -> Vec<u8> {
    let mut w = vec![0u32; K4_ALPHABET];
    for s in syms {
        w[s.0 as usize] = 1;
    }
    one_lane_gray8(width, height, 4, 0, &w, syms)
}
