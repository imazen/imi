//! Direct output mapping coverage; no process-wide backend overrides.
use super::*;
use crate::Channels;

fn samples(bits: u32) -> Vec<u32> {
    if bits <= 16 {
        return (0..1u32 << bits).collect();
    }
    let mask = crate::mask_of(bits);
    let mut values = vec![0, 1, mask, mask - 1, mask / 2, mask / 2 + 1];
    for bit in 0..bits {
        values.extend([1 << bit, mask ^ (1 << bit)]);
    }
    let mut state = 0x9e37_79b9u32;
    for _ in 0..521 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        values.push(state & mask);
    }
    values
}

fn expected(value: u32, mode: MapMode, bits: u8, base: u32, sample: SampleType) -> u32 {
    let depth = sample.depth();
    let expanded = match mode {
        MapMode::Shift => ((value as u64) << (depth - u32::from(bits))) as u32,
        MapMode::Offset => (u64::from(value) + u64::from(base)) as u32,
        _ => unreachable!(),
    };
    match sample {
        SampleType::F16 if expanded & 0x8000 != 0 => expanded ^ 0x7fff,
        SampleType::F32 if expanded & 0x8000_0000 != 0 => expanded ^ 0x7fff_ffff,
        SampleType::I32 => expanded ^ 0x8000_0000,
        _ => expanded,
    }
}

fn check<S: crate::px::Px>(sample: SampleType, map: ChannelMap) {
    let layout = Layout::new(Channels::Gray, vec![map]);
    let info = ImageInfo::new(263, 1, Channels::Gray, sample);
    let om = OutMap::new(&info, &layout);
    assert!(om.luts.iter().all(Option::is_none));
    let mut scalar = OutMap::new(&info, &layout);
    scalar.simd8 = None;
    scalar.simd16 = None;
    scalar.simd32 = None;
    for values in samples(u32::from(map.bits)).chunks(263) {
        let width = values.len();
        let lane_width = width.div_ceil(NL);
        let widths = core::array::from_fn(|lane| {
            width.saturating_sub(lane * lane_width).min(lane_width) as u32
        });
        let mut current = vec![vec![S::default(); lane_width * NL]];
        let mut want = Vec::new();
        for (pixel, &value) in values.iter().enumerate() {
            current[0][pixel % lane_width * NL + pixel / lane_width] = S::put(value);
            let v = expected(value, map.mode, map.bits, map.value, sample);
            match sample.bytes() {
                1 => want.push(v as u8),
                2 => want.extend_from_slice(&(v as u16).to_ne_bytes()),
                _ => want.extend_from_slice(&v.to_ne_bytes()),
            }
        }
        for writer in [&om, &scalar] {
            let mut got = vec![0xa7; want.len() + 31];
            write_output_row(
                writer,
                &current,
                &widths,
                lane_width,
                &mut got[..want.len()],
            );
            assert_eq!(&got[..want.len()], &want, "{sample:?} {map:?}");
            assert!(got[want.len()..].iter().all(|&v| v == 0xa7));
        }
    }
}

#[test]
fn algebraic_expansion_matches_scalar_and_dispatched_writers() {
    for sample in [
        SampleType::U8,
        SampleType::U16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
    ] {
        for bits in 1..=sample.depth() {
            let map = ChannelMap {
                mode: MapMode::Shift,
                bits: bits as u8,
                value: 0,
            };
            if bits <= 16 {
                check::<u16>(sample, map);
            } else {
                check::<u32>(sample, map);
            }
        }
        if sample.depth() == 32 {
            for bits in 0..=32 {
                for base in [0u32, 0x7fff_fff1, 0xffff_fff1] {
                    let map = ChannelMap {
                        mode: MapMode::Offset,
                        bits: bits as u8,
                        value: base,
                    };
                    if bits <= 16 {
                        check::<u16>(sample, map);
                    } else {
                        check::<u32>(sample, map);
                    }
                }
            }
        }
    }
}

#[test]
fn algebraic_maps_allocate_no_expansion_tables() {
    let info = ImageInfo::new(1, 1, Channels::Rgba, SampleType::U32);
    let modes = [
        MapMode::Shift,
        MapMode::Offset,
        MapMode::Replicate,
        MapMode::Scale,
    ];
    let maps = modes
        .map(|mode| ChannelMap {
            mode,
            bits: 16,
            value: 17,
        })
        .to_vec();
    let om = OutMap::new(&info, &Layout::new(Channels::Rgba, maps));
    assert!(om.luts[0].is_none() && om.luts[1].is_none());
    assert_eq!(om.luts[2].as_ref().unwrap().len(), 65536);
    assert_eq!(om.luts[3].as_ref().unwrap().len(), 65536);
    let bytes: usize = om
        .luts
        .iter()
        .flatten()
        .map(|v| v.len() * size_of::<u32>())
        .sum();
    // Previously all four maps allocated 256 KiB tables; the two algebraic
    // maps now need zero table payload. This is not process RSS measurement.
    assert_eq!(bytes, 512 * 1024);
}

#[test]
fn admission_lut_payload_matches_retained_capacity() {
    for (sample, mode, bits, value, entries) in [
        (SampleType::U8, MapMode::Lsb, 7, 0, 0),
        (SampleType::U16, MapMode::Lsb, 16, 0, 0),
        (SampleType::U16, MapMode::Const, 0, 91, 0),
        (SampleType::U16, MapMode::Shift, 12, 0, 0),
        (SampleType::U32, MapMode::Offset, 16, 37, 0),
        (SampleType::U16, MapMode::Replicate, 16, 0, 0),
        (SampleType::U16, MapMode::Scale, 16, 0, 0),
        (SampleType::U16, MapMode::Replicate, 12, 0, 4096),
        (SampleType::U16, MapMode::Scale, 15, 0, 32768),
        (SampleType::U32, MapMode::Replicate, 16, 0, 65536),
        (SampleType::U32, MapMode::Scale, 17, 0, 0),
        (SampleType::F32, MapMode::F16InF32, 16, 0, 65536),
        (SampleType::F32, MapMode::UnormF32, 8, 0, 256),
        (SampleType::F32, MapMode::UnormF32, 16, 0, 65536),
        (SampleType::F32, MapMode::UnormF32, 24, 0, 0),
    ] {
        let map = ChannelMap { mode, bits, value };
        let info = ImageInfo::new(1, 1, Channels::Gray, sample);
        let output = OutMap::new(&info, &Layout::new(Channels::Gray, vec![map]));
        let retained: usize = output.luts.iter().flatten().map(Vec::capacity).sum();
        assert_eq!(retained, entries, "{sample:?} {map:?}");
        assert_eq!(OutMap::lut_entries(&map, sample.depth()), retained);
        if let Some(table) = &output.luts[0] {
            for (coded, &expanded) in table.iter().enumerate() {
                assert_eq!(expanded, map.expand(coded as u32, sample.depth()));
            }
        }
    }
}

/// Value-table maps (SPEC 2.1 map mode 9) on every writer: each coded value
/// expands to its entry, and indices past the last entry to the last entry.
fn check_table<S: crate::px::Px>(sample: SampleType, table: &[u32]) {
    let bits = crate::mapping::table_bits(table.len());
    let map = ChannelMap {
        mode: MapMode::Table,
        bits,
        value: table.len() as u32 - 1,
    };
    let layout = Layout::new(Channels::Gray, vec![map]).with_tables(vec![table.to_vec()]);
    let info = ImageInfo::new(263, 1, Channels::Gray, sample);
    let om = OutMap::new(&info, &layout);
    assert_eq!(om.luts[0].as_ref().unwrap().len(), 1 << bits);
    // Admission covers all retained copies, rounded up to its u32 units.
    // Keep the primary-table length assertion above independently exact.
    let retained = om.luts.iter().flatten().map(Vec::capacity).sum::<usize>() * size_of::<u32>()
        + om.luts16().iter().map(Vec::capacity).sum::<usize>() * size_of::<u16>();
    #[cfg(all(feature = "avx512", target_arch = "x86_64"))]
    let retained = retained
        + om.tables16.as_ref().map_or(0, |t| {
            t.packed.iter().map(Vec::capacity).sum::<usize>() * size_of::<u32>()
        });
    assert_eq!(
        OutMap::lut_entries(&map, sample.depth()) * size_of::<u32>(),
        retained.div_ceil(size_of::<u32>()) * size_of::<u32>()
    );
    let mut scalar = OutMap::new(&info, &layout);
    scalar.simd8 = None;
    scalar.simd16 = None;
    scalar.simd32 = None;
    let values: Vec<u32> = (0..1u32 << bits).collect();
    for values in values.chunks(263) {
        let width = values.len();
        let lane_width = width.div_ceil(NL);
        let widths = core::array::from_fn(|lane| {
            width.saturating_sub(lane * lane_width).min(lane_width) as u32
        });
        let mut current = vec![vec![S::default(); lane_width * NL]];
        let mut want = Vec::new();
        for (pixel, &value) in values.iter().enumerate() {
            current[0][pixel % lane_width * NL + pixel / lane_width] = S::put(value);
            let ordered = table[(value as usize).min(table.len() - 1)];
            let v = sample.order(ordered);
            match sample.bytes() {
                1 => want.push(v as u8),
                2 => want.extend_from_slice(&(v as u16).to_ne_bytes()),
                _ => want.extend_from_slice(&v.to_ne_bytes()),
            }
        }
        for writer in [&om, &scalar] {
            let mut got = vec![0xa7; want.len() + 31];
            write_output_row(
                writer,
                &current,
                &widths,
                lane_width,
                &mut got[..want.len()],
            );
            assert_eq!(&got[..want.len()], &want, "{sample:?} {}", table.len());
            assert!(got[want.len()..].iter().all(|&v| v == 0xa7));
        }
    }
}

#[test]
fn value_tables_expand_on_every_writer_and_clamp_past_the_last_entry() {
    for sample in [
        SampleType::U8,
        SampleType::I8,
        SampleType::U16,
        SampleType::I16,
        SampleType::F16,
        SampleType::U32,
        SampleType::I32,
        SampleType::F32,
    ] {
        let top = crate::mask_of(sample.depth()) as u64;
        // One entry (a 0-bit channel), counts just past and below powers of
        // two (indices past the end), and the encoder's largest table.
        for count in [
            1usize,
            2,
            3,
            5,
            6,
            63,
            64,
            65,
            127,
            128,
            129,
            255,
            256,
            257,
            511,
            512,
            513,
            1000,
            1023,
            1024,
            1025,
            1 << 14,
            32767,
            32768,
            32769,
            65535,
            65536,
        ] {
            if count as u64 > top + 1 {
                continue;
            }
            // Ascending values spread over the container, both extremes.
            let table: Vec<u32> = (0..count as u64)
                .map(|i| {
                    if count == 1 {
                        (top / 3) as u32
                    } else {
                        (i * top / (count as u64 - 1)) as u32
                    }
                })
                .collect();
            assert!(table.windows(2).all(|p| p[0] < p[1]));
            check_table::<u16>(sample, &table);
            check_table::<u32>(sample, &table);
        }
    }
}

/// Exercise byte permutes (RGB/RGBA), every word-table bank boundary,
/// mixed algebraic/constant channels, and later blocks that leave the prefix
/// chosen from the first block. Compare against the SPEC's clamped lookup.
#[test]
fn value_tables_rgb_writers_and_adaptive_prefix_changes() {
    for sample in [
        SampleType::U8,
        SampleType::I8,
        SampleType::U16,
        SampleType::I16,
        SampleType::F16,
    ] {
        let top = crate::mask_of(sample.depth());
        for count in [
            1usize, 2, 3, 5, 6, 31, 64, 128, 256, 512, 1024, 1025, 32768, 65535, 65536,
        ] {
            if count as u64 > u64::from(top) + 1 {
                continue;
            }
            let bits = crate::mapping::table_bits(count);
            let table: Vec<u32> = (0..count)
                .map(|i| {
                    if count == 1 {
                        top / 3
                    } else {
                        (i as u64 * u64::from(top) / (count - 1) as u64) as u32
                    }
                })
                .collect();
            for channels in [Channels::Rgb, Channels::Rgba] {
                let nch = channels.count();
                for mixed in [false, true] {
                    let maps: Vec<ChannelMap> = (0..nch)
                        .map(|c| {
                            if mixed && c == 1 {
                                ChannelMap {
                                    mode: MapMode::Shift,
                                    bits: sample.depth() as u8,
                                    value: 0,
                                }
                            } else if mixed && c == 3 {
                                ChannelMap {
                                    mode: MapMode::Const,
                                    bits: 0,
                                    value: top / 3,
                                }
                            } else {
                                ChannelMap {
                                    mode: MapMode::Table,
                                    bits,
                                    value: count as u32 - 1,
                                }
                            }
                        })
                        .collect();
                    let tables = maps
                        .iter()
                        .map(|m| {
                            if m.mode == MapMode::Table {
                                table.clone()
                            } else {
                                Vec::new()
                            }
                        })
                        .collect();
                    let layout = Layout::new(channels, maps).with_tables(tables);
                    for width in [263usize, 1031] {
                        let info = ImageInfo::new(width as u32, 1, channels, sample);
                        let om = OutMap::new(&info, &layout);
                        let mut scalar = OutMap::new(&info, &layout);
                        scalar.simd8 = None;
                        scalar.simd16 = None;
                        scalar.simd32 = None;
                        let lw = width.div_ceil(NL);
                        let widths =
                            core::array::from_fn(|l| width.saturating_sub(l * lw).min(lw) as u32);
                        for first in [
                            [0; 8],
                            [256; 8],
                            [1024; 8],
                            [0, 0, 0, 0, 256, 256, 1024, 1024],
                            [0, 0, 256, 256, 1024, 1024, 1024, 1024],
                        ] {
                            let mut cur = vec![vec![0u16; lw * NL]; layout.slot_channel.len()];
                            let mut want = Vec::new();
                            for px in 0..width {
                                let (l, x) = (px / lw, px % lw);
                                for c in 0..nch {
                                    let map = layout.maps[c];
                                    let mask = crate::mask_of(u32::from(map.bits));
                                    let index = if x < 16 {
                                        first[l / 2] + x as u32
                                    } else if x == 16 {
                                        mask
                                    } else if x == 17 {
                                        mask.saturating_sub(1)
                                    } else {
                                        (px as u32 * 127) + c as u32 * 29
                                    } & mask;
                                    if let Some(s) = om.slot_of[c] {
                                        cur[s][x * NL + l] = index as u16;
                                    }
                                    let ordered = match map.mode {
                                        MapMode::Table => table[(index as usize).min(count - 1)],
                                        MapMode::Const => map.value,
                                        MapMode::Shift => index,
                                        _ => unreachable!(),
                                    };
                                    let v = sample.order(ordered);
                                    if sample.bytes() == 1 {
                                        want.push(v as u8);
                                    } else {
                                        want.extend_from_slice(&(v as u16).to_ne_bytes());
                                    }
                                }
                            }
                            for writer in [&om, &scalar] {
                                let mut got = vec![0xa7; want.len() + 31];
                                write_output_row(writer, &cur, &widths, lw, &mut got[..want.len()]);
                                assert_eq!(
                                    got[..want.len()],
                                    want,
                                    "{sample:?} {channels:?} {count} {width} {mixed} {first:?}"
                                );
                                assert!(got[want.len()..].iter().all(|&v| v == 0xa7));
                            }
                        }
                    }
                }
            }
        }
    }
}
