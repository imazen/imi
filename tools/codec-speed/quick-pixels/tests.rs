#![forbid(unsafe_code)]
#[path = "core.rs"]
mod codec;
use codec::{Shape, decode_into, encode, header};

fn raster(s: Shape, seed: u64) -> Vec<u8> {
    let mut rng = seed;
    let mut data = vec![0; s.row().unwrap() * s.height];
    for x in &mut data {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        *x = rng as u8;
    }
    data
}
fn check(s: Shape, raw: &[u8], qoi: Option<u8>) {
    let row = s.row().unwrap();
    let stride = row + 13;
    let mut input = vec![0x91; stride * s.height];
    for y in 0..s.height {
        input[y * stride..y * stride + row].copy_from_slice(&raw[y * row..(y + 1) * row]);
    }
    // Last-row padding is optional. Neither encoder nor decoder may access it.
    input.truncate((s.height - 1) * stride + row);
    let packed = || Ok(()).encode_samples(imij::ImageRef::new(raw, row, s), qoi).unwrap();
    let strided = || Ok(()).encode_samples(imij::ImageRef::new(&input, stride, s), qoi).unwrap();
    assert_eq!(packed, strided);
    assert_eq!(header(&packed).unwrap().0, s);
    let mut out = vec![0xd3; stride * s.height];
    let len = (s.height - 1) * stride + row;
    decode_into(&packed, &mut out[..len], stride, || Ok(())).unwrap();
    for y in 0..s.height {
        assert_eq!(
            &out[y * stride..y * stride + row],
            &raw[y * row..(y + 1) * row]
        );
        assert!(
            out[y * stride + row..(y + 1) * stride]
                .iter()
                .all(|&v| v == 0xd3)
        );
    }
    for i in 0..packed.len() {
        assert!(
            decode_into(&packed[..i], &mut out, stride, || Ok(())).is_err(),
            "truncation {i}"
        );
    }
    let mut extra = packed.clone();
    extra.insert(extra.len() - 8, 0);
    assert!(decode_into(&extra, &mut out, stride, || Ok(())).is_err());
}
#[test]
fn all_storage_widths_channels_strides_and_runs() {
    for bytes in [1, 2, 4, 8] {
        for channels in 1..=4 {
            for width in [1, 2, 61, 62, 63, 127] {
                let s = Shape {
                    width,
                    height: 3,
                    channels,
                    bytes,
                };
                let random = raster(s, 0x871347a334be);
                check(s, &random, None);
                check(s, &vec![0; s.row().unwrap() * s.height], None);
                check(s, &vec![255; s.row().unwrap() * s.height], None);
                if bytes == 1 && channels >= 3 {
                    check(s, &random, Some(0));
                }
            }
        }
    }
}
#[test]
fn signed_float_raw_bits_and_wide_modular_deltas() {
    for bytes in [1, 2, 4, 8] {
        for channels in 1..=4 {
            let words = [
                0u64,
                1,
                2,
                3,
                31,
                32,
                33,
                255,
                256,
                257,
                0x7c00,
                0xfc00,
                0x7e51,
                0x8000,
                0x7f800000,
                0xff800000,
                0x7fa12345,
                0x80000000,
                0x7ff0000000000000,
                0xfff0000000000000,
                0x7ff012345678abcd,
                0x8000000000000000,
                u64::MAX - 2,
                u64::MAX - 1,
                u64::MAX,
            ];
            let s = Shape {
                width: words.len(),
                height: 2,
                channels,
                bytes,
            };
            let mut raw = Vec::new();
            for _ in 0..2 {
                for &word in &words {
                    for _ in 0..channels {
                        raw.extend_from_slice(&word.to_le_bytes()[..bytes]);
                    }
                }
            }
            check(s, &raw, None);
        }
    }
}
#[test]
fn arbitrary_significant_depth_patterns_are_exact() {
    for bits in 1..=64 {
        let bytes = if bits <= 8 {
            1
        } else if bits <= 16 {
            2
        } else if bits <= 32 {
            4
        } else {
            8
        };
        let mask = u64::MAX >> (64 - bits);
        let s = Shape {
            width: 65,
            height: 1,
            channels: 4,
            bytes,
        };
        let mut raw = raster(s, 1234);
        for word in raw.chunks_exact_mut(bytes) {
            let mut v = [0; 8];
            v[..bytes].copy_from_slice(word);
            word.copy_from_slice(&(u64::from_le_bytes(v) & mask).to_le_bytes()[..bytes]);
        }
        check(s, &raw, None);
    }
}
#[test]
fn specification_packets_and_initial_run_index() {
    // Hand-written wire sequence: initial-black run, literal red, index of
    // initial black (53), DIFF +1,+1,+1, LUMA +10,+10,+10, explicit alpha.
    let mut file = b"qoif".to_vec();
    file.extend_from_slice(&6u32.to_be_bytes());
    file.extend_from_slice(&1u32.to_be_bytes());
    file.extend_from_slice(&[
        4, 1, 0xc0, 254, 255, 0, 0, 53, 0x7f, 0xaa, 0x88, 255, 9, 8, 7, 6,
    ]);
    file.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    let mut out = [0; 24];
    decode_into(&file, &mut out, 24, || Ok(())).unwrap();
    assert_eq!(
        out,
        [
            0, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 255, 1, 1, 1, 255, 11, 11, 11, 255, 9, 8, 7, 6
        ]
    );
    file[14] = 0xc6;
    assert!(decode_into(&file, &mut out, 24, || Ok(())).is_err());
}
#[test]
fn initial_index_reinserts_the_tuple_at_its_actual_hash() {
    // A literal with hash zero replaces slot zero. Reading any untouched
    // initial slot then yields the all-zero tuple, whose hash is zero: a
    // following INDEX 0 must observe that insertion, not the old literal.
    for bytes in [1, 2, 4, 8] {
        for channels in 1..=4 {
            for initial_slot in 1..64 {
                let s = Shape {
                    width: 2,
                    height: 3,
                    channels,
                    bytes,
                };
                let mut file = b"IQP1".to_vec();
                file.extend_from_slice(&2u32.to_le_bytes());
                file.extend_from_slice(&3u32.to_le_bytes());
                file.extend_from_slice(&[channels as u8, bytes as u8, 0, 0, 255]);
                for _ in 0..if channels <= 2 { 1 } else { 3 } {
                    file.extend_from_slice(&1u64.to_le_bytes()[..bytes]);
                }
                file.extend_from_slice(&51u64.to_le_bytes()[..bytes]);
                // 3*1 + 5*1 + 7*1 + 11*51 = 576, divisible by 64.
                file.extend_from_slice(&[initial_slot, 0, 0xc2]);
                file.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
                let row = s.row().unwrap();
                let stride = row + 5;
                let mut out = vec![0xa5; 2 * stride + row];
                decode_into(&file, &mut out, stride, || Ok(())).unwrap();
                let mut first = Vec::new();
                for c in 0..channels {
                    let value: u64 = if (channels == 2 && c == 1) || c == 3 {
                        51
                    } else {
                        1
                    };
                    first.extend_from_slice(&value.to_le_bytes()[..bytes]);
                }
                assert_eq!(&out[..first.len()], first);
                assert!(out[first.len()..row].iter().all(|&v| v == 0));
                for y in 1..3 {
                    assert!(out[y * stride..y * stride + row].iter().all(|&v| v == 0));
                }
                for y in 0..2 {
                    assert!(
                        out[y * stride + row..(y + 1) * stride]
                            .iter()
                            .all(|&v| v == 0xa5)
                    );
                }
            }
        }
    }
}

#[test]
fn admission_cancellation_and_gray_rejection() {
    let s = Shape {
        width: 9,
        height: 4,
        channels: 2,
        bytes: 8,
    };
    let raw = raster(s, 234);
    assert_eq!(
        || Err("cancel").encode_samples(imij::ImageRef::new(&raw, s.row().unwrap(), s), None).unwrap_err(),
        "cancel"
    );
    let file = || Ok(()).encode_samples(imij::ImageRef::new(&raw, s.row().unwrap(), s), None).unwrap();
    let mut out = vec![0x71; raw.len()];
    assert_eq!(
        decode_into(&file, &mut out, s.row().unwrap(), || Err("cancel")),
        Err("cancel")
    );
    assert!(out.iter().all(|&v| v == 0x71));
    let mut calls = 0;
    assert_eq!(
        decode_into(&file, &mut out, s.row().unwrap(), || {
            calls += 1;
            if calls == s.height + 1 {
                Err("final cancel")
            } else {
                Ok(())
            }
        }),
        Err("final cancel")
    );
    let mut wrong = file.clone();
    wrong[16] = 0x41;
    assert_eq!(
        decode_into(&wrong, &mut out, s.row().unwrap(), || Ok(())),
        Err("gray packet")
    );
    for shape in [
        Shape { width: 0, ..s },
        Shape { channels: 5, ..s },
        Shape { bytes: 3, ..s },
        Shape {
            width: usize::MAX,
            ..s
        },
    ] {
        assert!(|| Ok(()).encode_samples(imij::ImageRef::new(&raw, 0, shape), None).is_err());
    }
    assert!(|| Ok(()).encode_samples(imij::ImageRef::new(&raw, s.row().unwrap() - 1, s), None).is_err());
    assert!(|| Ok(()).encode_samples(imij::ImageRef::new(&raw, s.row().unwrap(), s), Some(0)).is_err());
    assert!(decode_into(&file, &mut out[..4], s.row().unwrap(), || Ok(())).is_err());
}
