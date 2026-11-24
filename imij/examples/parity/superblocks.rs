//! Superblock fixtures (SPEC 2.9.6): merged files of every shape (each shape
//! alone and all together) across channel layouts, sample types, both color
//! modes, both entropy coders, stripe heights, column groups, AQ and exact
//! blocks. Generating needs a `lab` build (the switch that merges outside the
//! Compact tier and restricts the shapes); `check` reads the set in any
//! build.
use super::*;

/// Smooth ramps (merge territory), a textured band and hard edges, as 8-bit
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
                let edge = if (x / 23 + y / 17) % 5 == 0 {
                    40.0
                } else {
                    0.0
                };
                px[(y * w + x) * n + c] = (ramp + tex + edge).clamp(0.0, 255.0) as u8;
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

pub fn generate(dir: &Path) {
    use imij::DecodeSpeed;
    let mut f = Fixtures::new(dir);
    let mut merged = [0usize; 8];
    let mut total = 0;
    let mut save = |f: &mut Fixtures,
                    name: String,
                    info: &ImageInfo,
                    px: &[u8],
                    cfg: &ImiEncoderConfig,
                    mask: u8| {
        let enc = cfg
            .encode_samples(
                imij::ImageRef::new(px, info.row_bytes(), info),
                &Metadata::default(),
            )
            .unwrap();
        let st = imij::lab::lossy_superblock_stats(&enc).unwrap();
        // No shape outside the allowed ones.
        for s in 0..3 {
            assert!(mask >> s & 1 != 0 || st[1 + s] == 0, "{name}: {st:?}");
        }
        if st[1] + st[2] + st[3] > 0 {
            merged[mask as usize] += 1;
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
    for mask in [7u8, 1, 2, 4] {
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
                    let q = [5.0f32, 30.0, 70.0, 90.0][k % 4];
                    let info = ImageInfo::new(w, h, ch, sample);
                    let px = samples(&info);
                    let mut cfg = imij::ImiEncoderConfig::lossy(q)
                        .tune_lossy(|t| t.color = color)
                        .tune_lossy(|t| t.entropy = entropy)
                        .tune_lossy(|t| t.stripe_block_rows = rows);
                    cfg = cfg.with_column_groups(Some(group));
                    cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
                    cfg = cfg.tune_lossy(|t| t.lab.switches.superblock_shapes = Some(mask));
                    let name = format!(
                        "sb-m{mask}-z{wi}-c{ci}-s{si}-col{}-e{}-r{rows}-g{group}-q{q}",
                        color as u8, entropy as u8
                    );
                    save(&mut f, name, &info, &px, &cfg, mask);
                }
            }
        }
    }
    // AQ (the DC image's fourth channel) and exact blocks (Compact, RGB(A)
    // 8-bit): every parent of a merge shares its block code.
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
            let mut cfg = imij::ImiEncoderConfig::lossy(q);
            cfg = cfg.tune_lossy(|t| {
                t.aq_gamma = 0.5;
            });
            cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
            save(&mut f, format!("sb-aq-z{i}-q{q}"), &info, &px, &cfg, 7);
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
            let mut cfg = imij::ImiEncoderConfig::lossy(q).with_decode_speed(DecodeSpeed::Compact);
            cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
            save(&mut f, format!("sb-exact-z{i}-q{q}"), &info, &page, &cfg, 7);
            // A reconstruction bias (Lossy 0.4) beside merges: 8x8 blocks
            // take it, merged blocks do not (SPEC 2.9).
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
                cfg = cfg.tune_lossy(|t| t.lab.switches.superblocks = Some(true));
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
                    format!("sb-bias-z{i}-e{e}-q{q}"),
                    &info,
                    &px,
                    &cfg,
                    7,
                );
            }
        }
    }
    eprintln!("superblock fixtures: {total}, merged per shape mask {merged:?}");
    for mask in [7usize, 1, 2, 4] {
        assert!(merged[mask] >= 20, "shape mask {mask}: {merged:?}");
    }
    f.finish();
}
