//! Value-table channel maps (SPEC 2.1, map mode 9): channels that use few of
//! their container's values code each sample's index among them.
#[cfg(feature = "std")]
mod common;
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::mapping::MapMode;
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::*;

/// The SPEC 2.1 order map of an 8-, 16- or 32-bit sample (an involution).
fn order(sample: SampleType, v: u32) -> u32 {
    match sample {
        SampleType::F16 => v ^ ((v >> 15) * 0x7fff),
        SampleType::F32 | SampleType::F64 => v ^ ((v >> 31) * 0x7fff_ffff),
        SampleType::I8 => v ^ 0x80,
        SampleType::I16 => v ^ 0x8000,
        SampleType::I32 | SampleType::I64 => v ^ 0x8000_0000,
        _ => v,
    }
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// `count` distinct ordered values of a `depth`-bit word, ascending.
fn value_set(rng: &mut Rng, depth: u32, count: usize) -> Vec<u32> {
    let mask = if depth == 32 {
        u32::MAX
    } else {
        (1 << depth) - 1
    };
    let mut set = std::collections::BTreeSet::new();
    while set.len() < count {
        set.insert(rng.next() & mask);
    }
    set.into_iter().collect()
}

/// A smooth index field drawn through a sparse random set of values per
/// coded word: what tone-mapping inverses and RGBE decodes look like to the
/// coder. `count` values per word; `t` moves the field (frames).
fn sparse(info: &ImageInfo, seed: u32, count: usize, t: u32) -> Vec<u8> {
    let (w, h) = (info.width as usize, info.height as usize);
    let nch = info.channels.count();
    let words = info.sample.bytes().div_ceil(4);
    let depth = 8 * info.sample.bytes().min(4) as u32;
    let mut rng = Rng(seed | 1);
    let sets: Vec<Vec<u32>> = (0..nch * words)
        .map(|_| value_set(&mut rng, depth, count))
        .collect();
    let mut out = Vec::with_capacity(info.row_bytes() * h);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nch {
                let mut word = |k: usize| {
                    let set = &sets[k * nch + c];
                    let f = (x * 5 + y * 3 + c * 17 + t as usize * 7) % (2 * set.len());
                    let i = if f < set.len() {
                        f
                    } else {
                        2 * set.len() - 1 - f
                    };
                    // A little noise in the index domain.
                    let i = (i + (rng.next() % 3) as usize).min(set.len() - 1);
                    set[i]
                };
                match info.sample.bytes() {
                    1 => out.push(order(info.sample, word(0)) as u8),
                    2 => out.extend_from_slice(&(order(info.sample, word(0)) as u16).to_ne_bytes()),
                    4 => out.extend_from_slice(&order(info.sample, word(0)).to_ne_bytes()),
                    _ => {
                        let hi = order(info.sample, word(0)) as u64;
                        let lo = word(1) as u64;
                        out.extend_from_slice(&(hi << 32 | lo).to_ne_bytes());
                    }
                }
            }
        }
    }
    out
}

/// Noise over every value: no channel leaves gaps a table could close.
fn dense(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    (0..info.checked_buffer_len().unwrap())
        .map(|_| rng.next() as u8)
        .collect()
}

fn samples() -> Vec<SampleType> {
    let mut s = vec![
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    if cfg!(feature = "extended") {
        s.extend([
            SampleType::I8,
            SampleType::I16,
            SampleType::U32,
            SampleType::I32,
            SampleType::U64,
            SampleType::I64,
            SampleType::F64,
        ]);
    }
    s
}

fn has_tables(file: &[u8]) -> bool {
    header(file)
        .unwrap()
        .features
        .contains(Feature::ValueTables)
}

/// The lab's encoder switches are process-wide: every test in this binary
/// that encodes holds this lock.
static ENCODER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn encoder() -> std::sync::MutexGuard<'static, ()> {
    ENCODER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every public decode route returns `raw`.
fn routes(file: &[u8], info: &ImageInfo, raw: &[u8]) {
    assert_eq!(decode(file).unwrap(), (info.clone(), raw.to_vec()));
    let rb = info.row_bytes();
    let stride = rb + 7;
    for threads in [1, 2, 4, 8] {
        let mut out = vec![0xa5; stride * info.height as usize];
        #[cfg(feature = "std")]
        decode_into_threads(file, &mut out, stride, threads).unwrap();
        #[cfg(not(feature = "std"))]
        {
            let _ = threads;
            decode_into(file, &mut out, stride).unwrap();
        }
        for (got, want) in out.chunks(stride).zip(raw.chunks(rb)) {
            assert_eq!(&got[..rb], want, "{threads} threads");
            assert!(got[rb..].iter().all(|&v| v == 0xa5));
        }
    }
    let mut stream = StreamDecoder::new(file).unwrap();
    let mut row = vec![0; rb];
    for want in raw.chunks(rb) {
        assert!(stream.next_row(&mut row).unwrap());
        assert_eq!(row, want);
    }
    assert!(!stream.next_row(&mut row).unwrap());
    let mut owned = FrameDecoder::from_owned(file.to_vec(), &Limits::DEFAULT).unwrap();
    owned.next_frame().unwrap().unwrap();
    let mut out = vec![0; raw.len()];
    owned.decode_into(&mut out, rb, 2).unwrap();
    assert_eq!(out, raw);
    let mut session = DecodeSession::new(DecodePolicy::default());
    assert_eq!(session.decode(file).unwrap().1, raw);
    let mut region = RegionReader::open(SliceReader::new(file)).unwrap();
    let rect = Rect::new(1, 1, info.width - 2, info.height - 2);
    let bpp = info.bytes_per_pixel();
    let crb = rect.width() as usize * bpp;
    let mut cropped = vec![0; crb * rect.height() as usize];
    region.decode_region(rect, &mut cropped, crb).unwrap();
    for y in 0..rect.height() as usize {
        let start = (y + 1) * rb + bpp;
        assert_eq!(&cropped[y * crb..(y + 1) * crb], &raw[start..start + crb]);
    }
    #[cfg(feature = "std")]
    {
        let mut read = Vec::new();
        decode_reader(file, &Limits::DEFAULT, |_, _, row| {
            read.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(read, raw);
    }
}

#[test]
fn sparse_channels_code_value_tables_on_every_sample_type_and_route() {
    let _lock = encoder();
    // 8-bit words keep regular maps (docs/VALUE_TABLES.md); see
    // `eight_bit_words_keep_regular_maps_unless_forced`.
    for sample in samples().into_iter().filter(|s| s.bytes() > 1) {
        for channels in [Channels::Gray, Channels::Rgb, Channels::Rgba] {
            let info = ImageInfo::new(97, 41, channels, sample);
            // Few values per word, with gaps in every container.
            let raw = sparse(&info, 7, 300, 0);
            for cfg in [
                imij::ImiEncoderConfig::default(),
                {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.with_column_groups(Some(32));
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(8);
                    });
                    cfg
                },
                imij::ImiEncoderConfig::default().with_effort(0.0),
            ] {
                let file = cfg
                    .encode_samples(
                        imij::ImageRef::new(&raw, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                assert!(has_tables(&file), "{sample:?} {channels:?}");
                assert!(
                    header(&file)
                        .unwrap()
                        .profiles
                        .iter()
                        .any(|p| p.profile == Profile::Scientific && p.minor == 2),
                    "{sample:?} {channels:?}"
                );
                routes(&file, &info, &raw);
                let off = {
                    let mut config = cfg;
                    config = config.tune_lossless(|t| {
                        t.value_tables = false;
                    });
                    config
                };
                let plain = off
                    .encode_samples(
                        imij::ImageRef::new(&raw, info.row_bytes(), &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                assert!(!has_tables(&plain), "{sample:?} {channels:?} off");
                assert!(file.len() < plain.len(), "{sample:?} {channels:?} smaller");
            }
        }
    }
}

/// The encoder writes no tables for 8-bit words by default; forced (lab),
/// every route decodes them.
#[test]
fn eight_bit_words_keep_regular_maps_unless_forced() {
    let _lock = encoder();
    for sample in samples().into_iter().filter(|s| s.bytes() == 1) {
        for channels in [Channels::Gray, Channels::Rgb, Channels::Rgba] {
            let info = ImageInfo::new(97, 41, channels, sample);
            let raw = sparse(&info, 7, 40, 0);
            let cfg = imij::ImiEncoderConfig::default();
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert!(!has_tables(&file), "{sample:?} {channels:?}");
            #[cfg(feature = "lab")]
            {
                imij::lab::set_value_tables(2);
                let forced = cfg.encode_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &Metadata::default(),
                );
                imij::lab::set_value_tables(0);
                let forced = forced.unwrap();
                assert!(has_tables(&forced), "{sample:?} {channels:?} forced");
                routes(&forced, &info, &raw);
            }
        }
    }
}

#[test]
fn tables_are_absent_on_dense_channels() {
    let _lock = encoder();
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ] {
        let info = ImageInfo::new(160, 96, Channels::Rgb, sample);
        let raw = dense(&info, 3);
        let file = imij::ImiEncoderConfig::default()
            .encode_samples(
                imij::ImageRef::new(&raw, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert!(!has_tables(&file), "{sample:?} dense");
        assert_eq!(decode(&file).unwrap().1, raw);
    }
}

#[test]
fn inter_frames_reduce_references_outside_their_tables() {
    let _lock = encoder();
    for (channels, sample) in [
        (Channels::Gray, SampleType::U16),
        (Channels::Rgb, SampleType::F16),
        (Channels::Rgba, SampleType::U16),
        (Channels::Rgb, SampleType::F32),
    ] {
        let (w, h) = (90u32, 60u32);
        let info = ImageInfo::new(w, h, channels, sample);
        // Each frame draws from its own value set, so most reference
        // samples fall between (or outside) the next frame's entries.
        let frames: Vec<Vec<u8>> = (0..5).map(|t| sparse(&info, 11 + t, 200, t)).collect();
        let mut cfg = FrameConfig::new(imij::ImiEncoderConfig::default());
        cfg.inter = true;
        let mut e = FrameEncoder::animation(info.clone(), Metadata::default(), cfg);
        for f in &frames {
            e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
                .unwrap();
        }
        let file = e.finish().unwrap();
        assert!(has_tables(&file), "{channels:?} {sample:?}");
        let mut d = FrameDecoder::new(&file).unwrap();
        let rb = info.row_bytes();
        let mut canvas = vec![0u8; rb * h as usize];
        let mut t = 0;
        while d.next_frame().unwrap().is_some() {
            d.decode_into(&mut canvas, rb, 3).unwrap();
            assert!(canvas == frames[t], "{channels:?} {sample:?} frame {t}");
            t += 1;
        }
        assert_eq!(t, frames.len());
    }
}

#[test]
fn near_lossless_channels_keep_regular_maps() {
    let _lock = encoder();
    let info = ImageInfo::new(97, 41, Channels::Rgb, SampleType::U16);
    let raw = sparse(&info, 5, 300, 0);
    let cfg = imij::ImiEncoderConfig::default().tune_lossless(|t| t.max_error = [0, 2, 0, 0]);
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    // The bounded channel (G) keeps a regular map; the exact ones may not.
    #[cfg(feature = "lab")]
    {
        let maps = imij::lab::channel_maps(&file).unwrap();
        assert_ne!(maps[1].0, format!("{:?}", MapMode::Table));
        assert_eq!(maps[0].0, format!("{:?}", MapMode::Table));
    }
    let back = decode(&file).unwrap().1;
    let words = |b: &[u8]| -> Vec<u16> {
        b.as_chunks::<2>()
            .0
            .iter()
            .map(|w| u16::from_ne_bytes(*w))
            .collect()
    };
    for (i, (a, b)) in words(&raw).into_iter().zip(words(&back)).enumerate() {
        let bound = if i % 3 == 1 { 2 } else { 0 };
        assert!(a.abs_diff(b) <= bound, "sample {i}: {a} vs {b}");
    }
}

/// The first channel map of a file's DATA payload (after presence, lanes,
/// stripe rows and column groups).
#[cfg(feature = "std")]
fn first_map(file: &[u8]) -> usize {
    let (_, at, _) = common::chunks(file)
        .into_iter()
        .find(|c| &c.0 == b"DATA")
        .unwrap();
    let (_, k) = common::varint(&file[at + 4..]);
    let mut p = at + 4 + k;
    p += common::varint(&file[p..]).1;
    p += 1;
    p += common::varint(&file[p..]).1;
    p += common::varint(&file[p..]).1;
    p
}

#[cfg(feature = "std")]
#[test]
fn malformed_value_tables_are_corrupt() {
    let _lock = encoder();
    // Gray f32: mode, bits, a u32 value, then the table's varints. Forty
    // ordered values from 2^31 (negative floats), so the first entry's
    // varint takes five bytes.
    let info = ImageInfo::new(64, 24, Channels::Gray, SampleType::F32);
    let raw: Vec<u8> = (0..64 * 24usize)
        .flat_map(|i| {
            let f = (i % 64 + i / 64) % 80;
            let k = if f < 40 { f } else { 79 - f } as u32;
            order(SampleType::F32, 0x8000_0000 + k * 4099).to_ne_bytes()
        })
        .collect();
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&raw, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let m = first_map(&file);
    assert_eq!(file[m], MapMode::Table as u8);
    assert_eq!(file[m + 1], 6, "40 entries take 6 index bits");
    let value = |f: &[u8]| u32::from_le_bytes(f[m + 2..m + 6].try_into().unwrap());
    assert_eq!(value(&file), 39);
    let corrupt = |f: Vec<u8>| match decode(&f) {
        Err(Error::Corrupt(m)) => m,
        other => panic!("{:?}", other.map(|d| d.0)),
    };
    // Index width other than the fewest bits that index every entry.
    for bits in [5u8, 7] {
        let mut f = file.clone();
        f[m + 1] = bits;
        assert_eq!(corrupt(f), "bad channel map");
    }
    // More entries than a table may have (65,536).
    let mut f = file.clone();
    f[m + 1] = 17;
    f[m + 2..m + 6].copy_from_slice(&65536u32.to_le_bytes());
    assert_eq!(corrupt(f), "bad channel map");
    // An entry past the container's largest value: the first entry's
    // five-byte varint takes its largest value, 2^35 - 1.
    let mut f = file.clone();
    assert_eq!(common::varint(&f[m + 6..]), (0x8000_0000, 5));
    for b in &mut f[m + 6..m + 10] {
        *b = 0xff;
    }
    f[m + 10] = 0x7f;
    assert_eq!(corrupt(f), "value table entry out of range");
    // Map mode 9 in a file that does not declare value_tables.
    let mask = common::feature_mask(&file);
    let rest = mask & !(1 << Feature::ValueTables.id());
    let f = common::with_feature_mask(&file, (rest != 0).then_some(rest));
    assert_eq!(corrupt(f), "value table without the feature value_tables");
}
