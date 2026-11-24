//! Bounded joint map selection for explicitly declared integer source ranges.
//!
//! Minimizing each channel's Lsb width can disable residual color decorrelation.
//! Compare one common-width candidate using the existing predictor and entropy
//! estimates. No full-file trial encoding or change to stored sample values.
use super::{AnalysisConfig, Layout, analyze_stripe_fast, seg_width};
use crate::mapping::{ChannelMap, MapMode};
use crate::{Channels, ImageInfo, SampleType};
use alloc::{vec, vec::Vec};

pub(super) fn choose_maps(
    info: &ImageInfo,
    cfg: &AnalysisConfig,
    planes: &[Vec<u32>],
    maps: &mut [ChannelMap],
) {
    let Some(bits) = common_bits(info, cfg, maps) else {
        return;
    };
    let mut common = maps.to_vec();
    for map in &mut common[..3] {
        map.bits = bits;
    }
    let original_layout = Layout::new(info.channels, maps.to_vec());
    let common_layout = Layout::new(info.channels, common);
    let width = info.width as usize;
    let windows = windows(width, info.height as usize);
    let depth = info.sample.depth();
    let original_cost = sample_cost(&original_layout, planes, width, depth, cfg, &windows);
    let common_cost = sample_cost(&common_layout, planes, width, depth, cfg, &windows);
    // This is an estimate, not a proof of the final stream size. Retain the
    // original maps for ties and small estimated gains.
    if common_cost < original_cost * 0.99 {
        maps.copy_from_slice(&common_layout.maps);
    }
}

fn common_bits(info: &ImageInfo, cfg: &AnalysisConfig, maps: &[ChannelMap]) -> Option<u8> {
    // Full-storage inputs keep their previous mapping/search policy. The mask
    // has already been reduced to the caller's effective effort budget.
    if info.channel_bits(0) == Some(info.sample.depth() as u8)
        || cfg.decor_mask & 0b1110 == 0
        || cfg.maps_only
    {
        return None;
    }
    #[cfg(feature = "lab")]
    if cfg.guide.is_some() {
        return None;
    }
    if !matches!(info.sample, SampleType::U8 | SampleType::U16)
        || !matches!(info.channels, Channels::Rgb | Channels::Rgba)
        || !info.significant_bits[..3]
            .iter()
            .all(|&b| b == info.significant_bits[0])
        || !maps[..3].iter().all(|m| m.mode == MapMode::Lsb)
    {
        return None;
    }
    let bits = maps[..3].iter().map(|m| m.bits).max()?;
    maps[..3].iter().any(|m| m.bits != bits).then_some(bits)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Window {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

fn windows(width: usize, height: usize) -> Vec<Window> {
    let (w, h) = (width.min(128), height.min(8));
    // Avoid span*i overflowing on large dimensions or 32-bit targets.
    let position = |span: usize, i: usize| (span / 3) * i + (span % 3) * i / 3;
    let mut result = Vec::with_capacity(4);
    for i in 0..4 {
        let window = Window {
            x: position(width - w, i),
            y: position(height - h, i),
            width: w,
            height: h,
        };
        if !result.contains(&window) {
            result.push(window);
        }
    }
    result
}

fn sample_cost(
    layout: &Layout,
    planes: &[Vec<u32>],
    width: usize,
    depth: u32,
    cfg: &AnalysisConfig,
    windows: &[Window],
) -> f64 {
    let mut cost = 0.0;
    for window in windows {
        // Raw ordered planes have not yet been mapped. Reduce every sampled
        // component, including alpha, into this candidate's coded domain.
        let cropped: Vec<Vec<u32>> = layout
            .slot_channel
            .iter()
            .map(|&c| {
                (window.y..window.y + window.height)
                    .flat_map(|row| {
                        planes[c][row * width + window.x..row * width + window.x + window.width]
                            .iter()
                            .map(|&v| layout.maps[c].reduce(v, depth))
                    })
                    .collect()
            })
            .collect();
        let sp: Vec<&[u32]> = cropped.iter().map(|p| p.as_slice()).collect();
        let mut residuals = vec![vec![0u32; window.width * window.height]; sp.len()];
        let mut views: Vec<&mut [u32]> = residuals.iter_mut().map(|r| r.as_mut_slice()).collect();
        // Each window is an independent prediction domain with the configured
        // lane count. Its boundary/lane phases are an explicit approximation.
        analyze_stripe_fast(
            layout,
            &sp,
            window.width,
            seg_width(window.width, cfg.lanes),
            0,
            window.height,
            cfg,
            &mut views,
            None,
        );
        cost += residuals
            .iter()
            .map(|r| crate::v1::est_bits(r))
            .sum::<f64>();
    }
    cost
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{EncodeConfig, Metadata};

    fn lsb(bits: u8) -> ChannelMap {
        ChannelMap {
            mode: MapMode::Lsb,
            bits,
            value: 0,
        }
    }

    #[test]
    fn bounded_windows_all_aspect_ratios() {
        for w in [1, 3, 127, 128, 129, 512, 16385, usize::MAX] {
            for h in [1, 3, 7, 8, 9, 512, 16385, usize::MAX] {
                let ws = windows(w, h);
                assert!((1..=4).contains(&ws.len()));
                assert!(ws.iter().map(|r| r.width * r.height).sum::<usize>() <= 4096);
                for r in ws {
                    assert!(r.x <= w - r.width && r.y <= h - r.height);
                }
            }
        }
        assert_eq!(windows(1, 1).len(), 1);
    }

    #[test]
    fn eligibility_respects_source_precision_maps_and_effective_effort() {
        let info = ImageInfo::new(67, 35, Channels::Rgb, SampleType::U16)
            .with_significant_bits(&[10; 3])
            .unwrap();
        let maps = [lsb(9), lsb(10), lsb(10)];
        let mut cfg = AnalysisConfig::default();
        assert_eq!(common_bits(&info, &cfg, &maps), Some(10));
        for (effort, allowed, eligible) in [
            (0.0, 15, true),
            (5.0, 15, true),
            (10.0, 1, false),
            (0.0, 4, true),
        ] {
            cfg.decor_mask = crate::effort::Effort::new(effort)
                .unwrap()
                .decorrelations(allowed);
            assert_eq!(common_bits(&info, &cfg, &maps).is_some(), eligible);
        }
        cfg.decor_mask = 15;
        cfg.maps_only = true;
        assert_eq!(common_bits(&info, &cfg, &maps), None);
        cfg.maps_only = false;
        assert_eq!(common_bits(&info, &cfg, &[lsb(10); 3]), None);
        let mut changed = maps;
        changed[0].mode = MapMode::Shift;
        assert_eq!(common_bits(&info, &cfg, &changed), None);
        let full = ImageInfo::new(67, 35, Channels::Rgb, SampleType::U16);
        assert_eq!(common_bits(&full, &cfg, &maps), None);
        let mut explicit_full = full;
        explicit_full.significant_bits = [16, 16, 16, 0];
        assert_eq!(common_bits(&explicit_full, &cfg, &maps), None);
        let mixed = info.clone().with_significant_bits(&[9, 10, 10]).unwrap();
        assert_eq!(common_bits(&mixed, &cfg, &maps), None);
        #[cfg(feature = "lab")]
        {
            cfg.guide = Some(alloc::sync::Arc::new(vec![]));
            assert_eq!(common_bits(&info, &cfg, &maps), None);
        }
    }

    fn fixture(alpha: usize) -> (Vec<u8>, Vec<Vec<u32>>, Vec<u32>) {
        let mut pixels = Vec::new();
        let mut planes = vec![Vec::new(); 4];
        let mut alpha_codes = Vec::new();
        for i in 0..67u32 * 35 {
            let mut h = i.wrapping_add(0x6d2b79f5);
            h = (h ^ (h >> 16)).wrapping_mul(0x7feb352d);
            h = (h ^ (h >> 15)).wrapping_mul(0x846ca68b);
            h ^= h >> 16;
            let r = h & 511;
            let (a, code) = match alpha {
                0 => (h & 65535, h & 65535),
                1 => ((h & 255) << 8, h & 255),
                2 => ((h & 255) * 257, h & 255),
                3 => (((h & 1023) * 65535 + 511) / 1023, h & 1023),
                _ => (65535, 0),
            };
            alpha_codes.push(code);
            for (c, value) in [r, (2 * r + 511) & 1023, (2 * r + 1023) & 1023, a]
                .into_iter()
                .enumerate()
            {
                planes[c].push(value);
                pixels.extend_from_slice(&(value as u16).to_ne_bytes());
            }
        }
        (pixels, planes, alpha_codes)
    }

    #[test]
    fn public_full_width_alias_preserves_encoded_bytes() {
        let (pixels, _, _) = fixture(0);
        let info = ImageInfo::new(67, 35, Channels::Rgba, SampleType::U16);
        let mut alias = info.clone();
        alias.significant_bits = [16; 4];
        let cfg = EncodeConfig::default().with_effort(10.0);
        let encode = |info: &ImageInfo| {
            crate::encode(&pixels, info.row_bytes(), info, &Metadata::default(), &cfg).unwrap()
        };
        assert_eq!(encode(&info), encode(&alias));
    }

    #[test]
    fn mapped_alpha_cost_uses_coded_values_and_storage_round_trips() {
        let info = ImageInfo::new(67, 35, Channels::Rgba, SampleType::U16)
            .with_significant_bits(&[10, 10, 10, 16])
            .unwrap();
        let cfg = AnalysisConfig {
            lanes: 2,
            ..Default::default()
        };
        for (alpha, expected_mode) in [
            MapMode::Lsb,
            MapMode::Shift,
            MapMode::Replicate,
            MapMode::Scale,
            MapMode::Const,
        ]
        .into_iter()
        .enumerate()
        {
            let (pixels, raw, codes) = fixture(alpha);
            let mut presence = crate::mapping::presence(16);
            for &v in &raw[3] {
                presence[v as usize] = true;
            }
            let alpha_map = crate::mapping::detect(&presence, 16, true);
            assert_eq!(alpha_map.mode, expected_mode);
            let layout = Layout::new(Channels::Rgba, vec![lsb(9), lsb(10), lsb(10), alpha_map]);
            let actual = sample_cost(&layout, &raw, 67, 16, &cfg, &windows(67, 35));
            let mut coded = raw.clone();
            coded[3] = codes;
            let mut coded_maps = layout.maps.clone();
            if expected_mode != MapMode::Const {
                coded_maps[3] = lsb(alpha_map.bits);
            }
            let expected = sample_cost(
                &Layout::new(Channels::Rgba, coded_maps),
                &coded,
                67,
                16,
                &cfg,
                &windows(67, 35),
            );
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "alpha {expected_mode:?}"
            );
            for effort in [0.0, 5.0, 10.0] {
                for decor_mask in [1, 4, 15] {
                    let mut encode_cfg = EncodeConfig::default().with_effort(effort);
                    encode_cfg.decor_mask = decor_mask;
                    let encoded = crate::encode(
                        &pixels,
                        info.row_bytes(),
                        &info,
                        &Metadata::default(),
                        &encode_cfg,
                    )
                    .unwrap();
                    let (decoded_info, decoded) = crate::decode(&encoded).unwrap();
                    assert_eq!(decoded_info, info);
                    assert_eq!(decoded, pixels);
                }
            }
        }
    }
}
