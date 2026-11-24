//! Frozen fixtures for cross-platform decode parity. Generate once, then check
//! the SAME files on every backend. `tools/decode_parity.py` adds SHA-256s.
//! Usage: parity generate DIR | parity check[-upgrade|-validate] DIR [THREADS]; also
//! `generate-extra DIR` and (lab builds) `generate-superblocks DIR` and
//! `generate-small DIR`.
#[cfg(feature = "encode")]
use imij::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata, SampleType};
use std::{fs, path::Path};

#[path = "parity/extra.rs"]
mod extra;
#[cfg(feature = "encode")]
#[path = "parity/pixelsum.rs"]
mod pixelsum;
#[cfg(feature = "encode")]
#[path = "parity/quick.rs"]
mod quick;
#[cfg(feature = "lab")]
#[path = "parity/small.rs"]
mod small;
#[cfg(feature = "lab")]
#[path = "parity/superblocks.rs"]
mod superblocks;

#[cfg(feature = "encode")]
const CHANNELS: [Channels; 4] = [
    Channels::Gray,
    Channels::GrayAlpha,
    Channels::Rgb,
    Channels::Rgba,
];
#[cfg(feature = "encode")]
const SAMPLES: [SampleType; 6] = [
    SampleType::U8,
    SampleType::U16,
    SampleType::F16,
    SampleType::U32,
    SampleType::I32,
    SampleType::F32,
];

#[cfg(feature = "encode")]
fn pixels(info: &ImageInfo, pattern: usize, lossy: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(info.row_bytes() * info.height as usize);
    let nch = info.channels.count();
    for y in 0..info.height as usize {
        for x in 0..info.width as usize {
            for c in 0..nch {
                let i = y * info.width as usize + x;
                let value = match pattern {
                    0 => 0,
                    1 => u32::MAX,
                    2 => (i + c * 83) as u32,
                    3 => {
                        if (x / 3 + y / 5 + c) % 2 == 0 {
                            0
                        } else {
                            u32::MAX
                        }
                    }
                    4 => 1u32 << ((i + c * 7) % 32),
                    5 => !(1u32 << ((i + c * 7) % 32)),
                    6 => ((x as u32).wrapping_mul(0x9e3779b9)
                        ^ (y as u32).wrapping_mul(0x85ebca6b))
                    .rotate_left(c as u32 * 7),
                    _ => [
                        0, 1, 0x7fff, 0x8000, 0xffff, 0x7fffffff, 0x80000000, 0xffffffff,
                        0x007fffff, 0x00800000, 0x7f800000, 0xff800000, 0x7fc01234, 0x80000001,
                    ][(i + c) % 14],
                };
                match info.sample {
                    SampleType::U8 => out.push(value as u8),
                    SampleType::U16 => out.extend_from_slice(&(value as u16).to_ne_bytes()),
                    SampleType::F16 => {
                        // Finite color input in lossy fixtures; lossless covers all bit patterns.
                        let v = if lossy {
                            0x2800 + (value as u16 & 0x1fff)
                        } else {
                            value as u16
                        };
                        out.extend_from_slice(&v.to_ne_bytes());
                    }
                    SampleType::F32 => {
                        let v = if lossy {
                            (value as u8 as f32 / 255.0).to_bits()
                        } else {
                            value
                        };
                        out.extend_from_slice(&v.to_ne_bytes());
                    }
                    _ => out.extend_from_slice(&value.to_ne_bytes()),
                }
            }
        }
    }
    out
}

#[cfg(feature = "encode")]
struct Fixtures<'a> {
    dir: &'a Path,
    count: usize,
    manifest: String,
}
#[cfg(feature = "encode")]
impl Fixtures<'_> {
    fn new(dir: &Path) -> Fixtures<'_> {
        fs::create_dir(dir).expect("fixture directory must be new");
        Fixtures {
            dir,
            count: 0,
            manifest: "name\twidth\theight\tchannels\tsample\tlossless\tkind\tframes\n".into(),
        }
    }

    fn finish(self) {
        fs::write(self.dir.join("fixtures.tsv"), &self.manifest).unwrap();
        eprintln!("generated {} frozen fixtures", self.count);
    }

    fn save(
        &mut self,
        name: &str,
        info: &ImageInfo,
        source: &[u8],
        encoded: Vec<u8>,
        lossless: bool,
    ) {
        let (got, expected) = imij::decode(&encoded).unwrap();
        assert_eq!(got, *info, "{name}: metadata");
        if lossless {
            assert_eq!(expected, source, "{name}: lossless samples");
        }
        if info.channels.has_alpha() {
            let bpp = info.bytes_per_pixel();
            let bps = info.sample.bytes();
            for (a, b) in source.chunks_exact(bpp).zip(expected.chunks_exact(bpp)) {
                assert_eq!(&a[bpp - bps..], &b[bpp - bps..], "{name}: alpha");
            }
        }
        fs::write(self.dir.join(format!("{name}.imi")), encoded).unwrap();
        fs::write(self.dir.join(format!("{name}.raw")), expected).unwrap();
        self.manifest.push_str(&format!(
            "{name}\t{}\t{}\t{:?}\t{:?}\t{lossless}\tstill\t\n",
            info.width, info.height, info.channels, info.sample
        ));
        self.count += 1;
    }
    fn lossless(&mut self, name: &str, info: &ImageInfo, pattern: usize, cfg: &ImiEncoderConfig) {
        let px = pixels(info, pattern, false);
        let enc = cfg
            .encode_samples(
                imij::ImageRef::new(&px, info.row_bytes(), info),
                &Metadata::default(),
            )
            .unwrap();
        self.save(name, info, &px, enc, true);
    }
}

#[cfg(feature = "encode")]
fn generate(dir: &Path) {
    let mut f = Fixtures::new(dir);
    // Force every spatial predictor/decorrelation pair. Disable palette so it
    // cannot replace the chosen RGB residual representation.
    for (si, sample) in SAMPLES.into_iter().enumerate() {
        let info = ImageInfo::new(37, 19, Channels::Rgb, sample);
        for pattern in 2..8 {
            for pred in 0..7 {
                for decor in 0..4 {
                    let cfg = {
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.tune_lossless(|t| {
                            t.predictor_mask = 1 << pred;
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.decor_mask = 1 << decor;
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.palette = false;
                        });
                        cfg = cfg.tune_lossless(|t| {
                            t.stripe_rows = Some(7);
                        });
                        cfg = cfg.with_column_groups(Some(16));
                        cfg
                    };
                    f.lossless(
                        &format!("predict-s{si}-p{pred}-d{decor}-k{pattern}"),
                        &info,
                        pattern,
                        &cfg,
                    );
                }
            }
        }
    }
    // Every U8 value and every U16/F16 bit pattern under each predictor.
    for (si, sample) in SAMPLES[..3].iter().copied().enumerate() {
        let info = ImageInfo::new(
            257,
            if sample == SampleType::U8 { 2 } else { 256 },
            Channels::Gray,
            sample,
        );
        for pred in 0..7 {
            let cfg = {
                let mut cfg = imij::ImiEncoderConfig::default();
                cfg = cfg.tune_lossless(|t| {
                    t.predictor_mask = 1 << pred;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.palette = false;
                });
                cfg = cfg.tune_lossless(|t| {
                    t.stripe_rows = Some(17);
                });
                cfg
            };
            f.lossless(&format!("all-values-s{si}-p{pred}"), &info, 2, &cfg);
        }
    }
    // Boundary and layout cross-product; lane counts rotate through 1..16.
    for (di, (w, h)) in [
        (1, 1),
        (7, 5),
        (8, 8),
        (9, 9),
        (15, 17),
        (16, 16),
        (17, 15),
        (31, 33),
        (32, 32),
        (33, 31),
        (255, 3),
        (256, 4),
        (257, 5),
    ]
    .into_iter()
    .enumerate()
    {
        for (ci, channels) in CHANNELS.into_iter().enumerate() {
            for (si, sample) in SAMPLES.into_iter().enumerate() {
                let info = ImageInfo::new(w, h, channels, sample);
                let cfg = {
                    let mut cfg = imij::ImiEncoderConfig::default();
                    cfg = cfg.tune_lossless(|t| {
                        t.lanes = Some(((di * 24 + ci * 6 + si) % 16 + 1) as u32);
                    });
                    cfg = cfg.tune_lossless(|t| {
                        t.stripe_rows = Some(3);
                    });
                    cfg
                };
                for pattern in [0, 1, 2, 3, 7] {
                    f.lossless(
                        &format!("geometry-{di}-c{ci}-s{si}-k{pattern}"),
                        &info,
                        pattern,
                        &cfg,
                    );
                }
            }
        }
    }
    for (si, sample) in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ]
    .into_iter()
    .enumerate()
    {
        for (ci, channels) in CHANNELS.into_iter().enumerate() {
            let info = ImageInfo::new(35, 19, channels, sample);
            for pattern in [0, 1, 2, 3, 4, 6] {
                // The eN name index is the entropy mode's wire value;
                // 1 (packed) and 2 (split) are retired — no fixture is
                // generated, sealed fixtures with those names are
                // rejection checks.
                for (ei, entropy) in [(0, LossyEntropy::RunLevel), (3, LossyEntropy::SplitHuf)] {
                    for quality in [1.0, 85.0, 116.0] {
                        for (col, color) in
                            [LossyColor::YCoCg, LossyColor::Xyb].into_iter().enumerate()
                        {
                            let mut cfg = imij::ImiEncoderConfig::lossy(quality)
                                .tune_lossy(|t| t.color = color)
                                .tune_lossy(|t| t.entropy = entropy);
                            cfg = cfg.with_column_groups(Some(16));
                            let px = pixels(&info, pattern, true);
                            let enc = cfg
                                .encode_samples(
                                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                                    &Metadata::default(),
                                )
                                .unwrap();
                            f.save(
                                &format!("lossy-s{si}-c{ci}-k{pattern}-e{ei}-q{quality}-col{col}"),
                                &info,
                                &px,
                                enc,
                                false,
                            );
                        }
                    }
                }
            }
        }
    }
    f.finish();
}

/// Retired entropy modes (packed 1, split 2): `lossy-*-eN-` carries the
/// wire value, `animation-*-eN-` the coding index (wire + 1). Sealed
/// fixtures with retired names decode to `Error::Unsupported`, never
/// pixels.
fn retired_entropy(name: &str) -> bool {
    let Some(e) = name.split('-').find_map(|p| p.strip_prefix('e')) else {
        return false;
    };
    let i: u32 = e.parse().unwrap_or(u32::MAX);
    (name.starts_with("lossy-") && (i == 1 || i == 2))
        || (name.starts_with("animation-") && (i == 2 || i == 3))
}

fn check(dir: &Path, threads: usize, upgrade: bool, validate: bool) {
    let manifest = fs::read_to_string(dir.join("fixtures.tsv")).unwrap();
    let mut checked = 0;
    for line in manifest.lines().skip(1) {
        let fields: Vec<_> = line.split('\t').collect();
        let name = fields[0];
        let file = fs::read(dir.join(format!("{name}.imi"))).unwrap();
        if retired_entropy(name) {
            assert!(
                matches!(imij::decode(&file), Err(imij::Error::Unsupported(_))),
                "{name}: retired entropy mode must be Unsupported"
            );
            checked += 1;
            continue;
        }
        let file = if upgrade {
            let upgraded = imij::upgrade_image(&file, &imij::UpgradeLimits::default())
                .unwrap_or_else(|e| panic!("{name}: upgrade: {e:?}"));
            assert_eq!(
                imij::header(&upgraded).unwrap().version,
                3,
                "{name}: upgraded version"
            );
            upgraded
        } else {
            file
        };
        let expected = fs::read(dir.join(format!("{name}.raw"))).unwrap();
        if validate {
            let mut scratch = std::io::Cursor::new(Vec::new());
            imij::validation::validate_v3(
                &mut imij::planes::SliceReader::new(&file),
                &mut scratch,
                &imij::validation::ValidationLimits::default(),
            )
            .unwrap_or_else(|e| panic!("{name}: strict validation: {e:?}"));
        }

        if fields.get(6).is_some_and(|&kind| kind != "still") {
            extra::check_frames(name, &file, &expected, fields[6], fields[7], threads);
            checked += 1;
            continue;
        }
        let (info, got) = imij::decode(&file).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(got, expected, "{name}: allocating decode");
        let rb = info.row_bytes();
        let stride = rb + 13;
        let mut padded = vec![0xa5; stride * info.height as usize];
        imij::decode_into_threads(&file, &mut padded, stride, threads).unwrap();
        for (row, expected) in padded.chunks_exact(stride).zip(expected.chunks_exact(rb)) {
            assert_eq!(&row[..rb], expected, "{name}: padded/threaded decode");
            assert!(
                row[rb..].iter().all(|&b| b == 0xa5),
                "{name}: padding overwritten"
            );
        }
        let mut decoder = imij::StreamDecoder::new(&file).unwrap();
        let mut row = vec![0; rb];
        for expected in expected.chunks_exact(rb) {
            assert!(decoder.next_row(&mut row).unwrap(), "{name}: missing row");
            assert_eq!(row, expected, "{name}: stream decode");
        }
        assert!(!decoder.next_row(&mut row).unwrap(), "{name}: extra row");
        checked += 1;
    }
    println!(
        "checked {checked} fixtures; upgrade {upgrade}; validate {validate}; reported tier {}; threads {threads}",
        imij::simd_tier()
    );
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let dir = Path::new(&args[2]);
    // IMIJ_LENGTH_PREFIXED=1 (lab builds): write payloads without the unit
    // table hint (SPEC 2.7), as a writer that cannot buffer its records
    // does. Generation runs on this thread.
    #[cfg(feature = "lab")]
    if std::env::var_os("IMIJ_LENGTH_PREFIXED").is_some() {
        imij::lab::set_length_prefixed_layout(true);
    }
    match args[1].as_str() {
        #[cfg(feature = "encode")]
        "generate" => generate(dir),
        #[cfg(feature = "encode")]
        "generate-extra" => extra::generate(dir),
        #[cfg(feature = "encode")]
        "generate-pixelsum" => pixelsum::generate(dir),
        #[cfg(feature = "encode")]
        "generate-quick" => quick::generate(dir),
        #[cfg(feature = "encode")]
        "generate-quick-bits" => quick::generate_bits(dir),
        #[cfg(feature = "lab")]
        "generate-superblocks" => superblocks::generate(dir),
        #[cfg(feature = "lab")]
        "generate-small" => small::generate(dir),
        "check" => check(
            dir,
            args.get(3).map_or(1, |s| s.parse().unwrap()),
            false,
            false,
        ),
        "check-validate" => check(dir, 1, true, true),
        "check-upgrade" => check(
            dir,
            args.get(3).map_or(1, |s| s.parse().unwrap()),
            true,
            false,
        ),
        _ => panic!(
            "usage: parity generate[-extra|-superblocks|-small] DIR | parity check[-upgrade|-validate] DIR [THREADS]"
        ),
    }
}
