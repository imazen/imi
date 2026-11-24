//! Complex plane pairs (plane roles 12 and 13): packed GrayAlpha pairs
//! decode to interleaved (re, im) samples, scalar pairs, and the pairing
//! rules (docs/PLANES_AND_ROI.md, "Complex planes").

use imij::planes::{
    LevelDescriptor, PlaneDescriptor, PlaneRole, PlaneSetBuilder, PlaneSetLimits, PlaneSetReader,
    Rect, SliceReader,
};
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn available(st: SampleType) -> bool {
    st.tier() == imij::Tier::Baseline || cfg!(feature = "extended")
}

/// Samples of content `kind` (1 smooth, 5 special bit patterns: extremes,
/// NaN payloads, infinities, signed zeros, subnormals) as raw bits of the
/// sample type's width, `ch.count()` per pixel.
fn image(
    w: u32,
    h: u32,
    ch: Channels,
    st: SampleType,
    kind: u8,
    seed: u64,
) -> (ImageInfo, Vec<u8>) {
    const F64: [u64; 8] = [
        0x7FF8_0000_DEAD_BEEF,
        0xFFF0_0000_0000_0000,
        0x8000_0000_0000_0000,
        0x0000_0000_0000_0001,
        0x7FEF_FFFF_FFFF_FFFF,
        0xFFF7_FFFF_FFFF_FFFF,
        0x3FF0_0000_0000_0000,
        0,
    ];
    const F32: [u64; 8] = [
        0x7FC0_1234,
        0xFF80_0000,
        0x8000_0000,
        1,
        0x7F7F_FFFF,
        0xFFBF_FFFF,
        0x3F80_0000,
        0,
    ];
    const INT: [u64; 8] = [
        1 << 63,
        u64::MAX >> 1,
        0,
        u64::MAX,
        1,
        (1 << 63) | 1,
        0x8000,
        0x7FFF,
    ];
    let info = ImageInfo::new(w, h, ch, st);
    let mut r = Rng(seed | 1);
    let mut px = Vec::with_capacity(info.row_bytes() * h as usize);
    for y in 0..h {
        for x in 0..w {
            for c in 0..ch.count() as u32 {
                let g = (x * 37 + y * 11 + c * 1000) as i64 - 3000 + (r.next() % 9) as i64;
                let i = ((x + 7 * y + 3 * c) % 8) as usize;
                let v = match (kind, st) {
                    (5, SampleType::F64) => F64[i],
                    (5, SampleType::F32) => F32[i],
                    (5, _) => INT[i] >> (64 - 8 * st.bytes()),
                    (_, SampleType::F32) => ((g as f32) * 0.01).to_bits() as u64,
                    (_, SampleType::F64) => ((g as f64) * 0.001).to_bits(),
                    _ => g as u64,
                };
                match st.bytes() {
                    1 => px.push(v as u8),
                    2 => px.extend_from_slice(&(v as u16).to_ne_bytes()),
                    4 => px.extend_from_slice(&(v as u32).to_ne_bytes()),
                    _ => px.extend_from_slice(&v.to_ne_bytes()),
                }
            }
        }
    }
    (info, px)
}

fn primary() -> Vec<u8> {
    let info = ImageInfo::new(8, 8, Channels::Gray, SampleType::U8);
    imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&[3; 64], 8, &info),
            &Metadata::default(),
        )
        .unwrap()
}

fn pair(b: &mut PlaneSetBuilder, st: SampleType, group: u32) -> [imij::planes::PlaneId; 2] {
    [PlaneRole::ComplexReal, PlaneRole::ComplexImaginary].map(|role| {
        let name = if role == PlaneRole::ComplexReal {
            "field.re"
        } else {
            "field.im"
        };
        b.add_plane(
            PlaneDescriptor::new(name, st)
                .with_role(role)
                .with_group(group),
        )
        .unwrap()
    })
}

/// Interleaved complex samples (re, im per pixel), like numpy complex.
fn interleaved(st: SampleType, w: u32, h: u32) -> Vec<u8> {
    let (_, px) = image(w, h, Channels::GrayAlpha, st, 1, 17);
    px
}

#[test]
fn packed_pair_decodes_interleaved() {
    for st in [
        SampleType::I16,
        SampleType::I32,
        SampleType::F32,
        SampleType::F64,
    ] {
        if !available(st) {
            continue;
        }
        let (w, h) = (90, 50);
        let px = interleaved(st, w, h);
        let row = w as usize * 2 * st.bytes();
        let mut b = PlaneSetBuilder::new(&primary()).unwrap();
        let [re, im] = pair(&mut b, st, 1);
        b.add_color_level(
            &[re, im],
            LevelDescriptor::new(w, h, 32, 32),
            &px,
            row,
            Channels::GrayAlpha,
            imij::ImiEncoderConfig::default(),
        )
        .unwrap();
        let file = b.finish().unwrap();
        let mut r =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        assert_eq!(r.planes()[0].role(), &PlaneRole::ComplexReal);
        assert_eq!(r.planes()[1].role(), &PlaneRole::ComplexImaginary);
        let mut out = vec![0u8; px.len()];
        r.decode_color_region(&[re, im], 0, Rect::new(0, 0, w, h), &mut out, row)
            .unwrap();
        assert!(out == px, "{st:?}");
        // A region of the imaginary part alone.
        let sb = st.bytes();
        let mut part = vec![0u8; 10 * 7 * sb];
        r.decode_region(im, 0, Rect::new(33, 20, 10, 7), &mut part, 10 * sb)
            .unwrap();
        for y in 0..7 {
            for x in 0..10 {
                let src = (20 + y) * row + (33 + x) * 2 * sb + sb;
                assert_eq!(&part[(y * 10 + x) * sb..][..sb], &px[src..src + sb]);
            }
        }
    }
}

#[test]
fn scalar_pair_and_rules() {
    let st = SampleType::I16;
    let (_, re_px) = image(40, 30, Channels::Gray, st, 1, 1);
    let (_, im_px) = image(40, 30, Channels::Gray, st, 5, 2);
    let lossless = || imij::ImiEncoderConfig::default();
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let [re, im] = pair(&mut b, st, 0);
    b.add_level(
        re,
        LevelDescriptor::new(40, 30, 16, 16),
        &re_px,
        80,
        lossless(),
    )
    .unwrap();
    b.add_level(
        im,
        LevelDescriptor::new(40, 30, 40, 30),
        &im_px,
        80,
        lossless(),
    )
    .unwrap();
    let file = b.finish().unwrap();
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    for (id, px) in [(re, &re_px), (im, &im_px)] {
        let mut out = vec![0u8; px.len()];
        let result = r.decode_region(id, 0, Rect::new(0, 0, 40, 30), &mut out, 80);
        if available(st) {
            result.unwrap();
            assert!(out == *px);
        } else {
            assert!(matches!(result, Err(Error::Unsupported(_))));
            assert!(out.iter().all(|&v| v == 0));
        }
    }

    // A real part without its imaginary part.
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("re", st).with_role(PlaneRole::ComplexReal))
        .unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(40, 30, 16, 16),
        &re_px,
        80,
        lossless(),
    )
    .unwrap();
    assert!(matches!(b.finish(), Err(Error::InvalidInput(_))));
    // Parts of different sizes, or two real parts in one group.
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let [re, im] = pair(&mut b, st, 0);
    b.add_level(
        re,
        LevelDescriptor::new(40, 30, 16, 16),
        &re_px,
        80,
        lossless(),
    )
    .unwrap();
    b.add_level(
        im,
        LevelDescriptor::new(20, 30, 16, 16),
        &im_px,
        80,
        lossless(),
    )
    .unwrap();
    assert!(matches!(b.finish(), Err(Error::InvalidInput(_))));
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let [re, im] = pair(&mut b, st, 0);
    let re2 = b
        .add_plane(PlaneDescriptor::new("re2", st).with_role(PlaneRole::ComplexReal))
        .unwrap();
    for id in [re, im, re2] {
        b.add_level(
            id,
            LevelDescriptor::new(40, 30, 16, 16),
            &re_px,
            80,
            lossless(),
        )
        .unwrap();
    }
    assert!(matches!(b.finish(), Err(Error::InvalidInput(_))));
    // Complex levels are lossless.
    let mut b = PlaneSetBuilder::new(&primary()).unwrap();
    let [re, _] = pair(&mut b, SampleType::U16, 0);
    let r = b.add_level(
        re,
        LevelDescriptor::new(40, 30, 16, 16),
        &re_px,
        80,
        imij::ImiEncoderConfig::lossy(90.0),
    );
    assert!(matches!(r, Err(Error::InvalidInput(_))), "{r:?}");
}
