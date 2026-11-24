//! Independent wide arithmetic for reference predictors and residual colors.
use super::*;
use crate::px::Px;

fn check<S: Px>(bits: u32, pred: u8, decor: u8, pattern: usize) {
    let mask = crate::mask_of(bits);
    let modulus = 1i128 << bits;
    let wrap = |v: i128| v.rem_euclid(modulus) as u32;
    let signed = |v: u32| {
        if v as i128 >= modulus / 2 {
            v as i128 - modulus
        } else {
            v as i128
        }
    };
    let anchors = [0, 1, mask / 2, mask / 2 + 1, mask.saturating_sub(1), mask];
    let value = |s: usize, i: usize, offset: usize| {
        if pattern == 0 {
            anchors[(i + s + offset) % anchors.len()]
        } else {
            (i as u32)
                .wrapping_mul(0x9e3779b9)
                .rotate_left((s * 7 + offset) as u32)
                & mask
        }
    };
    let plane = |offset| -> Vec<Vec<u32>> {
        (0..3)
            .map(|s| (0..3 * NL).map(|i| value(s, i, offset)).collect())
            .collect()
    };
    let residual = plane(1);
    let previous = plane(2);
    let guide = plane(3);
    let guide_previous = plane(4);
    let mut expected = vec![vec![0u32; 3 * NL]; 3];
    for i in 0..3 * NL {
        let mut d: [u32; 3] = core::array::from_fn(|s| {
            let r = residual[s][i];
            wrap(if r & 1 == 0 {
                (r / 2) as i128
            } else {
                -((r / 2) as i128) - 1
            })
        });
        if decor != 0 {
            d[1] = wrap(d[1] as i128 + d[0] as i128);
            let add = match decor {
                1 => d[0] as i128,
                2 => d[1] as i128,
                3 => (signed(d[1]) + signed(d[0])).div_euclid(2),
                _ => unreachable!(),
            };
            d[2] = wrap(d[2] as i128 + add);
        }
        for s in 0..3 {
            let g = guide[s][i] as i128;
            let p = match pred {
                7 => g,
                8 if i >= NL => expected[s][i - NL] as i128 + g - guide[s][i - NL] as i128,
                8 | 9 => previous[s][i] as i128 + g - guide_previous[s][i] as i128,
                _ => unreachable!(),
            };
            expected[s][i] = wrap(p + d[s] as i128);
        }
    }
    let layout = Layout::new(
        crate::Channels::Rgb,
        vec![
            ChannelMap {
                mode: MapMode::Lsb,
                bits: bits as u8,
                value: 0,
            };
            3
        ],
    );
    let narrow = |v: &[Vec<u32>]| -> Vec<Vec<S>> {
        v.iter()
            .map(|r| r.iter().map(|&v| S::put(v)).collect())
            .collect()
    };
    // Three steps, all lanes full width: every position is covered.
    let widths = [3u32; NL];
    let mut out = vec![vec![S::default(); 3 * NL]; 3];
    let refs = RefRows {
        cur: narrow(&guide),
        prev: narrow(&guide_previous),
        need: Vec::new(),
    };
    recon_row_inter(
        &layout,
        &narrow(&residual),
        &narrow(&previous),
        &mut out,
        &refs,
        &widths,
        pred,
        decor,
    );
    let got: Vec<Vec<u32>> = out
        .iter()
        .map(|r| r.iter().map(|v| v.get()).collect())
        .collect();
    assert_eq!(
        got, expected,
        "bits={bits} predictor={pred} decor={decor} pattern={pattern}"
    );
    #[cfg(target_arch = "x86_64")]
    if bits <= 16 {
        use archmage::SimdToken;
        if let Some(t) = archmage::X64V3Token::summon() {
            let u16s = |v: &[Vec<u32>]| -> Vec<Vec<u16>> {
                v.iter()
                    .map(|r| r.iter().map(|&v| v as u16).collect())
                    .collect()
            };
            let (res16, prev16) = (u16s(&residual), u16s(&previous));
            let (gc16, gp16) = (u16s(&guide), u16s(&guide_previous));
            let mut out16 = vec![vec![0u16; 3 * NL]; 3];
            let r: [&[u16]; 3] = core::array::from_fn(|s| res16[s].as_slice());
            let pv: [&[u16]; 3] = core::array::from_fn(|s| prev16[s].as_slice());
            let gc: [&[u16]; 3] = core::array::from_fn(|s| gc16[s].as_slice());
            let gp: [&[u16]; 3] = core::array::from_fn(|s| gp16[s].as_slice());
            let mut it = out16.iter_mut();
            let mut c: [&mut [u16]; 3] =
                core::array::from_fn(|_| it.next().unwrap().as_mut_slice());
            let bits3 = [bits; 3];
            use crate::recon::avx2::recon_row_inter as rr;
            match pred {
                7 => rr::<3, { predict::P_REF }>(t, &bits3, 3, &r, &pv, &mut c, &gc, &gp, 3, decor),
                9 => {
                    rr::<3, { predict::P_REF_N }>(t, &bits3, 3, &r, &pv, &mut c, &gc, &gp, 3, decor)
                }
                _ => {
                    rr::<3, { predict::P_REF_W }>(t, &bits3, 3, &r, &pv, &mut c, &gc, &gp, 3, decor)
                }
            }
            let got16: Vec<Vec<u32>> = out16
                .iter()
                .map(|r| r.iter().map(|&v| v as u32).collect())
                .collect();
            assert_eq!(
                got16, expected,
                "AVX2 bits={bits} predictor={pred} decor={decor} pattern={pattern}"
            );
        }
    }
}

/// The AVX2 `ref_row` transpose must equal the scalar masked-sample loop.
#[test]
#[cfg(target_arch = "x86_64")]
fn ref_row16_matches_scalar() {
    use archmage::SimdToken;
    let Some(t) = archmage::X64V3Token::summon() else {
        return;
    };
    for lw in [16usize, 32, 48] {
        for bits in [4u32, 10, 12, 16] {
            let mask = crate::mask_of(bits) as u16;
            let w = lw * NL;
            let (mut row, mut d) = (vec![0u8; w * 2], vec![0u16; w]);
            for seed in 0..8u32 {
                for (i, b) in row.iter_mut().enumerate() {
                    *b = (i as u32)
                        .wrapping_mul(0x9e3779b9)
                        .wrapping_add(seed)
                        .rotate_left(seed % 13) as u8;
                }
                crate::recon::out_avx2::ref_row16(t, &row, lw, mask, &mut d);
                for l in 0..NL {
                    for x in 0..lw {
                        let raw =
                            u16::from_ne_bytes([row[(l * lw + x) * 2], row[(l * lw + x) * 2 + 1]]);
                        assert_eq!(d[x * NL + l], raw & mask, "lw={lw} bits={bits} seed={seed}");
                    }
                }
            }
        }
    }
}

#[test]
fn reference_predictors_all_depths_and_decorrelations() {
    for bits in 1..=32 {
        for pred in 7..=9 {
            for decor in 0..4 {
                for pattern in 0..2 {
                    check::<u32>(bits, pred, decor, pattern);
                    if bits <= 16 {
                        check::<u16>(bits, pred, decor, pattern);
                    }
                }
            }
        }
    }
}

/// Short final lanes leave empty lanes whose start lies past the output row.
#[test]
fn reference_rows_with_empty_trailing_lanes() {
    for w in [1usize, 15, 17, 232, 257, 320] {
        let info = ImageInfo::new(w as u32, 1, crate::Channels::Gray, SampleType::U16);
        let row: Vec<u8> = (0..w)
            .flat_map(|i| (i as u16).wrapping_mul(197).to_ne_bytes())
            .collect();
        let data = crate::encode(
            &row,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &EncodeConfig::default(),
        )
        .unwrap();
        let mut p = parse(&data).unwrap();
        p.layout = Layout::new(
            crate::Channels::Gray,
            vec![ChannelMap {
                mode: MapMode::Lsb,
                bits: 16,
                value: 0,
            }],
        );
        let lw = w.div_ceil(NL);
        let widths = core::array::from_fn(|l| w.saturating_sub(l * lw).min(lw) as u32);
        let mut dst = vec![vec![0u16; lw * NL]];
        ref_row(&p, &row, &widths, lw, &mut dst);
        for (l, &width) in widths.iter().enumerate() {
            for x in 0..width as usize {
                assert_eq!(
                    dst[0][x * NL + l],
                    ((l * lw + x) as u16).wrapping_mul(197),
                    "width={w} lane={l} x={x}"
                );
            }
        }
    }
}

#[test]
fn rgb8_reference_rows_with_empty_trailing_lanes() {
    for w in [1usize, 15, 17, 232, 257, 320] {
        let info = ImageInfo::new(w as u32, 1, crate::Channels::Rgb, SampleType::U8);
        let row: Vec<u8> = (0..w * 3).map(|i| i.wrapping_mul(197) as u8).collect();
        let data = crate::encode(
            &row,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &EncodeConfig::default(),
        )
        .unwrap();
        let mut p = parse(&data).unwrap();
        p.layout = Layout::new(
            crate::Channels::Rgb,
            vec![
                ChannelMap {
                    mode: MapMode::Lsb,
                    bits: 8,
                    value: 0,
                };
                3
            ],
        );
        let lw = w.div_ceil(NL);
        let widths = core::array::from_fn(|l| w.saturating_sub(l * lw).min(lw) as u32);
        let mut dst = vec![vec![0u16; lw * NL]; 3];
        ref_row(&p, &row, &widths, lw, &mut dst);
        assert_eq!(dst.len(), 3);
        let mut channels = p.layout.slot_channel.clone();
        channels.sort_unstable();
        assert_eq!(channels, [0, 1, 2]);
        for (s, channel) in dst.iter().enumerate() {
            let c = p.layout.slot_channel[s];
            for (l, &width) in widths.iter().enumerate() {
                for x in 0..width as usize {
                    assert_eq!(
                        channel[x * NL + l],
                        row[(l * lw + x) * 3 + c] as u16,
                        "width={w} slot={s} channel={c} lane={l} x={x}"
                    );
                }
            }
        }
    }
}
