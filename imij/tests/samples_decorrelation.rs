//! Forced residual decorrelation (SPEC 2.4) on i8, i16 and 64-bit RGB and
//! RGBA. With `decor_mask = 1 << d` the encoder may only use mode `d`, so
//! every stripe record must carry it, and the image must decode exactly
//! through every decode path. For 64-bit samples the color slots are the hi
//! words; both 32-bit row storage (full-width words) and 16-bit row storage
//! (every coded word within 16 bits) are covered.

mod common;

use imij::{Channels, ImageInfo, Metadata, SampleType, StreamDecoder, decode};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The decorrelation byte (SPEC 2.4: a record's first byte) of every stripe
/// record of a lossless file, in either payload layout.
fn stripe_modes(file: &[u8], info: &ImageInfo) -> Vec<u8> {
    let d = &file[common::data_payload(file)];
    common::lossless_payload(d, info)
        .records
        .iter()
        .map(|r| d[r.start])
        .collect()
}

fn put(px: &mut Vec<u8>, st: SampleType, v: u64) {
    match st.bytes() {
        1 => px.push(v as u8),
        2 => px.extend_from_slice(&(v as u16).to_ne_bytes()),
        _ => px.extend_from_slice(&v.to_ne_bytes()),
    }
}

/// G noise, R = G + 3 and B = G + 5 (in the hi word for 64-bit samples, so
/// the three hi words have equal maps), alpha independent noise. `narrow`
/// keeps every 64-bit word within 15 bits (16-bit row storage).
fn image(
    w: u32,
    h: u32,
    ch: Channels,
    st: SampleType,
    narrow: bool,
    seed: u64,
) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, ch, st);
    let mut r = Rng(seed | 1);
    let step = if st.bytes() == 8 { 1u64 << 32 } else { 1 };
    let mut sample = || {
        let v = r.next();
        if narrow {
            ((v >> 17) & 0x7FFF) << 32 | (v & 0x7FFF)
        } else {
            v
        }
    };
    let mut px = Vec::with_capacity(info.row_bytes() * h as usize);
    for _ in 0..w * h {
        let g = sample();
        for v in [g.wrapping_add(3 * step), g, g.wrapping_add(5 * step)] {
            put(&mut px, st, v);
        }
        if ch.has_alpha() {
            put(&mut px, st, sample());
        }
    }
    (info, px)
}

/// Every decode path returns `px`.
fn check(file: &[u8], info: &ImageInfo, px: &[u8]) {
    if !cfg!(feature = "extended") && info.sample.tier() == imij::Tier::Extended {
        assert!(matches!(decode(file), Err(imij::Error::Unsupported(_))));
        assert!(matches!(
            StreamDecoder::new(file),
            Err(imij::Error::Unsupported(_))
        ));
        return;
    }
    let (got_info, got) = decode(file).unwrap();
    assert_eq!(got_info, *info);
    assert!(got == px, "decode");
    let row = info.row_bytes();
    let stride = row + 7;
    for threads in [1, 4] {
        let mut out = vec![0x5a; stride * info.height as usize];
        imij::decode_into_threads(file, &mut out, stride, threads).unwrap();
        for (y, expected) in px.chunks_exact(row).enumerate() {
            assert!(
                out[y * stride..y * stride + row] == *expected,
                "{threads} threads"
            );
        }
    }
    let mut stream = StreamDecoder::new(file).unwrap();
    let mut out = vec![0; row];
    for expected in px.chunks_exact(row) {
        assert!(stream.next_row(&mut out).unwrap());
        assert!(out == expected, "stream");
    }
    let mut rows = Vec::new();
    imij::decode_reader(file, &imij::Limits::DEFAULT, |_, _, r| {
        rows.extend_from_slice(r);
        Ok(())
    })
    .unwrap();
    assert!(rows == px, "reader");
}

#[test]
fn every_forced_decorrelation_mode_is_used_and_exact() {
    let mut cases = vec![(SampleType::I8, false), (SampleType::I16, false)];
    if cfg!(feature = "extended") {
        for st in [SampleType::U64, SampleType::I64, SampleType::F64] {
            cases.extend([(st, false), (st, true)]);
        }
    }
    for (st, narrow) in cases {
        for ch in [Channels::Rgb, Channels::Rgba] {
            let (info, px) = image(150, 40, ch, st, narrow, 0x9e37 + st as u64);
            for mode in 1..=3u8 {
                for (rows, lanes) in [(8, None), (3, Some(16))] {
                    let cfg = {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.decor_mask = 1 << mode;
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.stripe_rows = Some(rows);
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.lanes = lanes;
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.palette = false;
                        });
                        cfg
                    };
                    let file = cfg
                        .encode_legacy_samples(
                            imij::ImageRef::new(&px, info.row_bytes(), &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    common::assert_current_migration_pixels(&file);
                    let modes = stripe_modes(&file, &info);
                    assert_eq!(modes.len(), 40usize.div_ceil(rows as usize));
                    assert!(
                        modes.iter().all(|&m| m == mode),
                        "{st:?} {ch:?} narrow={narrow} mode {mode}: stripes used {modes:?}"
                    );
                    check(&file, &info, &px);
                }
            }
        }
    }
}
