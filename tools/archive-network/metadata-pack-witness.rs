//! Length-prefixed stdio driver for the exact metadata pack implementation.
#![allow(dead_code)]

extern crate alloc;

#[path = "../../imij/src/metadata_pack.rs"]
mod metadata_pack;
#[path = "../codec-speed/quick-pixels/core.rs"]
mod qoi;

use std::io::{self, Read, Write};

#[derive(Debug)]
enum Error {
    Corrupt(&'static str),
    Unsupported(&'static str),
    TooLarge,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

fn main() -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut length = [0u8; 4];
    while input.read(&mut length[..1])? != 0 {
        input.read_exact(&mut length[1..])?;
        let header = u32::from_le_bytes(length);
        let qoi_input = header >> 31 != 0;
        let size = (header & 0x7fff_ffff) as usize;
        assert!(size <= 65536);
        let mut raw = vec![0u8; size];
        input.read_exact(&mut raw)?;
        let packed = if qoi_input {
            assert!(size.is_multiple_of(3));
            let shape = qoi::Shape {
                width: size / 3,
                height: 1,
                channels: 3,
                bytes: 1,
            };
            let packed = qoi::encode(&raw, size, shape, Some(0), || Ok(())).unwrap();
            let mut decoded = vec![0; size];
            qoi::decode_into(&packed, &mut decoded, size, || Ok(())).unwrap();
            assert_eq!(decoded, raw);
            packed
        } else {
            let packed = metadata_pack::pack(&raw).unwrap();
            assert_eq!(metadata_pack::unpack(&packed, 65536).unwrap(), raw);
            packed
        };
        output.write_all(&(packed.len() as u32).to_le_bytes())?;
        output.write_all(&packed)?;
        output.flush()?;
    }
    Ok(())
}
