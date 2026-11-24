//! Output writers for i8, i16 and 64-bit samples: the dispatched SIMD writer
//! and the scalar writer agree for every channel layout and map mix.
use super::*;
use crate::Channels;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A random exact map for coded channel `cc` (alpha and 64-bit lo words
/// may be Const). `narrow` keeps every coded width at most 16 bits.
fn random_map(r: &mut Rng, depth: u32, may_const: bool, narrow: bool) -> ChannelMap {
    let max_bits = if narrow { depth.min(16) } else { depth };
    let bits = 1 + r.below(max_bits as u64) as u8;
    let pick = r.below(if may_const { 6 } else { 5 });
    let (mode, bits, value) = match pick {
        0 => (MapMode::Lsb, bits, 0),
        1 => (MapMode::Shift, bits, 0),
        2 if depth == 32 => (MapMode::Offset, bits, r.next() as u32),
        2 => (MapMode::Lsb, depth.min(max_bits) as u8, 0),
        3 => (MapMode::Replicate, bits.min(16), 0),
        4 => (MapMode::Scale, bits.min(16), 0),
        _ => (MapMode::Const, 0, (r.next() as u32) & crate::mask_of(depth)),
    };
    ChannelMap { mode, bits, value }
}

fn check<S: crate::px::Px>(info: &ImageInfo, maps: Vec<ChannelMap>, r: &mut Rng) {
    let layout = Layout::new(info.channels, maps);
    let om = OutMap::new(info, &layout);
    let mut scalar = OutMap::new(info, &layout);
    scalar.simd8 = None;
    scalar.simd16 = None;
    scalar.simd32 = None;
    let width = info.width as usize;
    let lane_width = width.div_ceil(NL);
    let widths: [u32; NL] =
        core::array::from_fn(|l| width.saturating_sub(l * lane_width).min(lane_width) as u32);
    let cur: Vec<Vec<S>> = layout
        .slot_bits
        .iter()
        .map(|&b| {
            (0..lane_width * NL)
                .map(|_| S::put(r.next() as u32 & crate::mask_of(b)))
                .collect()
        })
        .collect();
    let n = info.row_bytes();
    let mut want = vec![0x5a; n + 17];
    write_output_row(&scalar, &cur, &widths, lane_width, &mut want[..n]);
    let mut got = vec![0x5a; n + 17];
    write_output_row(&om, &cur, &widths, lane_width, &mut got[..n]);
    assert!(
        got == want,
        "{:?} {:?} {:?}",
        info.sample,
        info.channels,
        layout.maps
    );
}

#[test]
fn new_sample_writers_match_scalar() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    for sample in [
        SampleType::I8,
        SampleType::I16,
        SampleType::U64,
        SampleType::I64,
        SampleType::F64,
    ] {
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for width in [1u32, 15, 16, 17, 263] {
                for trial in 0..24 {
                    let info = ImageInfo::new(width, 1, channels, sample);
                    let nch = channels.count();
                    let depth = sample.word_depth();
                    // Every other trial keeps all coded words within 16 bits
                    // (u16 row storage).
                    let narrow = trial % 2 == 0 || depth <= 16;
                    let maps: Vec<ChannelMap> = (0..nch * sample.words())
                        .map(|cc| {
                            let alpha = channels.has_alpha() && cc % nch == nch - 1;
                            random_map(&mut r, depth, alpha || cc >= nch, narrow)
                        })
                        .collect();
                    if narrow {
                        check::<u16>(&info, maps, &mut r);
                    } else {
                        check::<u32>(&info, maps, &mut r);
                    }
                }
            }
        }
    }
}

/// The 64-bit writer joins the words as `(order(hi) << 32) | lo`, the i8 and
/// i16 writers flip the sign bit, for known values.
#[test]
fn new_sample_writer_known_values() {
    let cases: [(SampleType, [u32; 2], &[u8]); 5] = [
        (SampleType::I8, [0x00, 0], &[0x80]),
        (SampleType::I16, [0x7FFF, 0], &0xFFFFu16.to_ne_bytes()),
        (
            SampleType::U64,
            [0xDEAD_BEEF, 0x0123_4567],
            &0xDEAD_BEEF_0123_4567u64.to_ne_bytes(),
        ),
        (
            SampleType::I64,
            [0x7FFF_FFFF, 0xFFFF_FFFF],
            &(-1i64).to_ne_bytes(),
        ),
        (
            SampleType::F64,
            [0x7FFF_FFFF ^ 0xBFF0_0000, 1],
            &(-1.000_000_000_000_000_2f64).to_ne_bytes(),
        ),
    ];
    for (sample, [hi, lo], want) in cases {
        let words = sample.words();
        let depth = sample.word_depth();
        let maps = vec![ChannelMap::identity(depth); words];
        let info = ImageInfo::new(1, 1, Channels::Gray, sample);
        let layout = Layout::new(Channels::Gray, maps);
        for scalar in [false, true] {
            let mut om = OutMap::new(&info, &layout);
            if scalar {
                om.simd8 = None;
                om.simd16 = None;
                om.simd32 = None;
            }
            let mut out = vec![0u8; sample.bytes()];
            let mut widths = [0u32; NL];
            widths[0] = 1;
            fn rows<S: crate::px::Px>(v: &[u32]) -> Vec<Vec<S>> {
                v.iter()
                    .map(|&v| {
                        let mut row = vec![S::default(); NL];
                        row[0] = S::put(v);
                        row
                    })
                    .collect()
            }
            if depth <= 16 {
                write_output_row(&om, &rows::<u16>(&[hi, lo][..words]), &widths, 1, &mut out);
            } else {
                write_output_row(&om, &rows::<u32>(&[hi, lo][..words]), &widths, 1, &mut out);
            }
            assert_eq!(out, want, "{sample:?} scalar={scalar}");
        }
    }
}
