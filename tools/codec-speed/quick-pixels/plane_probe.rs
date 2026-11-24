//! Qualification tool for exact byte planes; not an IMI file encoder.
#![forbid(unsafe_code)]
extern crate alloc;
#[path = "../../../imij/src/quick/pixels.rs"]
mod pixels;
#[path = "planes.rs"]
mod planes;
use pixels::Shape;
use std::{fs, hint::black_box, time::Instant};

fn decode(encoded: &[u8], s: Shape, out: &mut [u8]) -> Result<(), &'static str> {
    let mut state = planes::State::new(s, encoded)?;
    for row in out.chunks_exact_mut(s.row()?) {
        state.next(encoded, row)?;
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 && a.len() != 5 {
        return Err("plane_probe RAW GEOMETRY OUTPUT [simple]".into());
    }
    let g: Vec<usize> = fs::read_to_string(&a[2])?
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if g.len() != 4 {
        return Err("geometry".into());
    }
    let s = Shape {
        width: g[0],
        height: g[1],
        channels: g[2],
        bytes: g[3],
    };
    let raw = fs::read(&a[1])?;
    let start = Instant::now();
    let encoded = if a.len() == 5 && a[4] == "simple" {
        planes::encode_variant(&raw, s.row()?, s, false, || Ok(()))?
    } else {
        planes::encode(&raw, s.row()?, s, || Ok(()))?
    };
    let encode_ns = start.elapsed().as_nanos();
    let mut out = vec![0; raw.len()];
    for _ in 0..3 {
        decode(&encoded, s, &mut out)?;
    }
    if out != raw {
        return Err("warmup pixels".into());
    }
    let start = Instant::now();
    for _ in 0..10 {
        decode(black_box(&encoded), s, black_box(&mut out))?;
    }
    let decode_ns = start.elapsed().as_nanos() / 10;
    if out != raw {
        return Err("pixels".into());
    }
    fs::write(&a[3], &encoded)?;
    println!(
        "{{\"bytes\":{},\"encode_ns\":{encode_ns},\"decode_ns\":{decode_ns},\"pixels\":{}}}",
        encoded.len(),
        s.width * s.height
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_storage_bits_strides_and_masks() {
        for bytes in [1, 2, 4, 8] {
            for channels in 1..=4 {
                let s = Shape {
                    width: 63,
                    height: 3,
                    bytes,
                    channels,
                };
                let row = s.row().unwrap();
                for mask in [0, 1, 3, 7, 15, 31, 63, 127, 255, 0xa5] {
                    let raw: Vec<u8> = (0..row * s.height)
                        .map(|i| ((i * 73) as u8 & mask) | !mask)
                        .collect();
                    let mut padded = vec![0x93; (row + 11) * s.height];
                    for (a, b) in padded.chunks_exact_mut(row + 11).zip(raw.chunks_exact(row)) {
                        a[..row].copy_from_slice(b);
                    }
                    let enc = planes::encode(&raw, row, s, || Ok(())).unwrap();
                    let state = planes::State::new(s, &enc).unwrap();
                    assert!(
                        (state.heap_bytes() as u128)
                            <= planes::working_bytes(
                                s.width as u128,
                                (s.channels * s.bytes) as u128
                            )
                            .unwrap()
                    );
                    assert_eq!(
                        enc,
                        planes::encode(&padded[..padded.len() - 11], row + 11, s, || Ok(()))
                            .unwrap()
                    );
                    let mut out = vec![0; raw.len()];
                    decode(&enc, s, &mut out).unwrap();
                    assert_eq!(out, raw);
                    for n in 0..enc.len() {
                        assert!(planes::State::new(s, &enc[..n]).is_err());
                    }
                    let mut tail = enc.clone();
                    tail.push(0);
                    assert!(planes::State::new(s, &tail).is_err());
                }
            }
        }
    }
    #[test]
    fn malformed_masks_and_sticky_packet_failure() {
        let s = Shape {
            width: 1,
            height: 1,
            channels: 1,
            bytes: 1,
        };
        for payload in [
            vec![2, 4, 3, 0, 0, 0],
            vec![2, 4, 3, 1, 1, 0],
            vec![2, 4, 3, 0, 1, 2],
            vec![2, 1, 99],
            vec![2, 0],
            vec![2, 0x81, 0, 0],
        ] {
            assert!(planes::State::new(s, &payload).is_err());
        }
        let payload = [2, 1, 1];
        let mut st = planes::State::new(s, &payload).unwrap();
        let first = st.next(&payload, &mut [0]).unwrap_err();
        assert_eq!(st.next(&payload, &mut [0]).unwrap_err(), first);
    }
}
