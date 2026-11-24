//! Small-transform fixtures (SPEC 2.11.5): files with 4x4 planes, forced on
//! every plane with AC, forced on luma or on the chroma planes, and priced,
//! alone and beside merges, across channel layouts, sample types, both
//! color modes, both entropy coders, stripe heights, column groups, AQ,
//! exact blocks, a reconstruction bias (8x8 planes take it, 4x4 planes do
//! not) and the effort-10 trellis. Generating needs a `lab` build (the
//! switches that turn small transforms on and force planes); `check` reads
//! the set in any build.
use super::*;

/// Smooth ramps, a textured band, fine detail and hard edges, as 8-bit
/// samples.
fn base(w: usize, h: usize, n: usize) -> Vec<u8> {
    let mut px = vec![0u8; w * h * n];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                let ramp = (x as f32 * 0.7 + y as f32 * 0.4 + c as f32 * 40.0) % 200.0 + 20.0;
                let tex = if x > w / 2 {
                    18.0 * ((x as f32 * 0.9 + c as f32).sin() * (y as f32 * 0.7).cos())
                } else {
                    0.0
                };
                // Colored fine detail (one channel only): AC in the chroma
                // planes too.
                let fine = if (x / 3 + y / 2) % 2 == 0 && y > h / 3 && c == n.min(2) - 1 {
                    45.0
                } else {
                    0.0
                };
                let edge = if (x / 23 + y / 17) % 5 == 0 {
                    40.0
                } else {
                    0.0
                };
                px[(y * w + x) * n + c] = (ramp + tex + fine + edge).clamp(0.0, 255.0) as u8;
            }
        }
    }
    px
}

/// `base` in sample type `sample` (16-bit: x 257; floats: linear 0..1).
fn samples(info: &ImageInfo) -> Vec<u8> {
    let b = base(
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    match info.sample {
        SampleType::U16 => b
            .iter()
            .flat_map(|&v| (v as u16 * 257).to_ne_bytes())
            .collect(),
        SampleType::F16 => b
            .iter()
            .flat_map(|&v| (0x2800u16 + v as u16 * 16).to_ne_bytes())
            .collect(),
        SampleType::F32 => b
            .iter()
            .flat_map(|&v| (v as f32 / 255.0).to_ne_bytes())
            .collect(),
        _ => b,
    }
}

/// The lab switches of arm `arm`: 0 every plane with AC 4x4, 1 luma 4x4,
/// 2 the chroma planes 4x4, 3 priced, 4 priced beside merges.
fn arm(cfg: &mut ImiEncoderConfig, arm: usize) {
    let split = [Some(7u8), Some(1), Some(6), None, None][arm];
    *cfg = cfg.clone().tune_lossy(|t| {
        t.lab.switches.small = Some(Box::new(imij::SmallLab {
            split,
            ..imij::SmallLab::on()
        }));
        t.lab.switches.superblocks = Some(arm == 4);
    });
}

pub fn generate(dir: &Path) {
    use imij::DecodeSpeed;
    let mut f = Fixtures::new(dir);
    let mut split_files = [0usize; 5];
    let mut merged_files = 0usize;
    let mut total = 0;
    let mut save = |f: &mut Fixtures,
                    name: String,
                    info: &ImageInfo,
                    px: &[u8],
                    cfg: &ImiEncoderConfig,
                    a: usize| {
        let enc = cfg
            .encode_samples(
                imij::ImageRef::new(px, info.row_bytes(), info),
                &Metadata::default(),
            )
            .unwrap();
        let st = imij::lab::lossy_small_stats(&enc).unwrap();
        // Forced arms split only the planes they force.
        match a {
            1 => assert!(st[3] == 0 && st[4] == 0, "{name}: {st:?}"),
            2 => assert!(st[2] == 0, "{name}: {st:?}"),
            _ => {}
        }
        if a != 4 {
            assert_eq!(st[1], 0, "{name}: merges without the switch");
        }
        if st[2] + st[3] + st[4] > 0 {
            split_files[a] += 1;
        }
        if st[1] > 0 {
            merged_files += 1;
        }
        total += 1;
        f.save(&name, info, px, enc, false);
    };
    let sizes = [(72u32, 56u32), (61, 45), (40, 136)];
    let channels = [
        Channels::Rgb,
        Channels::Rgba,
        Channels::Gray,
        Channels::GrayAlpha,
    ];
    let samples_t = [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::F32,
    ];
    let mut k = 0usize;
    for a in 0..5 {
        for (wi, &(w, h)) in sizes.iter().enumerate() {
            for (ci, &ch) in channels.iter().enumerate() {
                for (si, &sample) in samples_t.iter().enumerate() {
                    // A rotation over the other axes (a covering of their
                    // pairs across the set).
                    k += 1;
                    let float = matches!(sample, SampleType::F16 | SampleType::F32);
                    let color = if float || k.is_multiple_of(2) {
                        LossyColor::Xyb
                    } else {
                        LossyColor::YCoCg
                    };
                    let entropy = if (k / 2).is_multiple_of(2) {
                        LossyEntropy::RunLevel
                    } else {
                        LossyEntropy::SplitHuf
                    };
                    let rows = [2u32, 1, 3, 8][(k / 4) % 4];
                    let group = [0u32, 24, 16][(k / 3) % 3];
                    let q = [20.0f32, 50.0, 80.0, 97.0][k % 4];
                    let info = ImageInfo::new(w, h, ch, sample);
                    let px = samples(&info);
                    let mut cfg = imij::ImiEncoderConfig::lossy(q)
                        .tune_lossy(|t| t.color = color)
                        .tune_lossy(|t| t.entropy = entropy)
                        .tune_lossy(|t| t.stripe_block_rows = rows);
                    cfg = cfg.with_column_groups(Some(group));
                    arm(&mut cfg, a);
                    let name = format!(
                        "st-a{a}-z{wi}-c{ci}-s{si}-col{}-e{}-r{rows}-g{group}-q{q}",
                        color as u8, entropy as u8
                    );
                    save(&mut f, name, &info, &px, &cfg, a);
                }
            }
        }
    }
    // AQ, exact blocks (Compact, RGB(A) 8-bit), a reconstruction bias and
    // the effort-10 trellis beside 4x4 planes.
    for (i, (w, h, ch)) in [
        (72u32, 56u32, Channels::Rgb),
        (61, 45, Channels::Rgba),
        (128, 96, Channels::Rgb),
    ]
    .into_iter()
    .enumerate()
    {
        for q in [10.0f32, 40.0, 85.0] {
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let px = samples(&info);
            for a in [0usize, 4] {
                let mut cfg = imij::ImiEncoderConfig::lossy(q);
                cfg = cfg.tune_lossy(|t| {
                    t.aq_gamma = 0.5;
                });
                arm(&mut cfg, a);
                save(&mut f, format!("st-aq-a{a}-z{i}-q{q}"), &info, &px, &cfg, a);
                let mut cfg = imij::ImiEncoderConfig::lossy(q).with_effort(10.0);
                arm(&mut cfg, if a == 4 { 3 } else { 0 });
                save(
                    &mut f,
                    format!("st-e10-a{a}-z{i}-q{q}"),
                    &info,
                    &px,
                    &cfg,
                    if a == 4 { 3 } else { 0 },
                );
            }
            // Text-like ink on a flat page above the ramp: exact blocks.
            let mut page = px.clone();
            let n = ch.count();
            for y in 0..h as usize / 2 {
                for x in 0..w as usize {
                    let ink = (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11);
                    let o = (y * w as usize + x) * n;
                    page[o..o + 3].copy_from_slice(if ink {
                        &[20, 30, 90]
                    } else {
                        &[250, 250, 245]
                    });
                }
            }
            for a in [0usize, 4] {
                let mut cfg =
                    imij::ImiEncoderConfig::lossy(q).with_decode_speed(DecodeSpeed::Compact);
                arm(&mut cfg, a);
                save(
                    &mut f,
                    format!("st-exact-a{a}-z{i}-q{q}"),
                    &info,
                    &page,
                    &cfg,
                    a,
                );
            }
            for (e, entropy) in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf]
                .into_iter()
                .enumerate()
            {
                let mut cfg = imij::ImiEncoderConfig::lossy(q)
                    .tune_lossy(|t| {
                        t.color = if e == 0 {
                            LossyColor::Xyb
                        } else {
                            LossyColor::YCoCg
                        }
                    })
                    .tune_lossy(|t| t.entropy = entropy);
                arm(&mut cfg, 0);
                cfg = cfg.tune_lossy(|t| {
                    t.lab.switches.recon_bias = Some(imij::ReconBias {
                        k: 10,
                        b: [
                            [[64, 48], [32, 16]],
                            [[48, 32], [24, 12]],
                            [[-24, 8], [16, -8]],
                        ],
                    })
                });
                save(
                    &mut f,
                    format!("st-bias-z{i}-e{e}-q{q}"),
                    &info,
                    &px,
                    &cfg,
                    0,
                );
            }
        }
    }
    eprintln!(
        "small-transform fixtures: {total}, with 4x4 planes per arm {split_files:?}, with merges {merged_files}"
    );
    // Gray files have no chroma AC: arm 2 splits in RGB(A) files only.
    for (a, &n) in split_files.iter().enumerate() {
        assert!(
            n >= if a == 2 { 12 } else { 20 },
            "arm {a}: {split_files:?}"
        );
    }
    assert!(merged_files >= 10, "{merged_files} files with merges");
    f.finish();
}
