//! Value-table decode cost against table size (docs/VALUE_TABLES.md).
//!
//! `value_table_sizes OUTDIR SIDE` writes, for each table size K, a square
//! SIDE x SIDE RGB f16 image whose channels each use K scattered half
//! values along a smooth index field with a little noise (`k<K>.raw`), and
//! that image coded with a value table on every channel (`k<K>.table.imi`)
//! and with regular maps only (`k<K>.off.imi`). Every file is decoded and
//! compared before it is written. Time them with `lowbit_eval time`.
//! Lab builds only (`--features lab`): the encoder's table cap and choice
//! are lab switches.
use std::collections::BTreeSet;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let side: u32 = a[2].parse().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let info = imij::ImageInfo::new(side, side, imij::Channels::Rgb, imij::SampleType::F16);
    imij::lab::set_value_table_cap(1 << 16);
    for k in [256usize, 1024, 4096, 16384, 32768, 49152] {
        let mut s = 0x2545_f491u32 ^ k as u32;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s
        };
        // K ordered values per channel; the order map inverse makes halves.
        let sets: Vec<Vec<u16>> = (0..3)
            .map(|_| {
                let mut v = BTreeSet::new();
                while v.len() < k {
                    v.insert(next() as u16);
                }
                v.into_iter().map(|o| o ^ ((o >> 15) * 0x7fff)).collect()
            })
            .collect();
        let n = side as usize;
        let mut raw = Vec::with_capacity(info.row_bytes() * n);
        for y in 0..n {
            for x in 0..n {
                for (c, set) in sets.iter().enumerate() {
                    // A smooth sweep over the whole set across the image.
                    let f = (x * 3 + y * 2 + c * 97) * k / (5 * n) % (2 * k);
                    let i = if f < k { f } else { 2 * k - 1 - f };
                    let i = (i + (next() % 5) as usize).min(k - 1);
                    raw.extend_from_slice(&set[i].to_ne_bytes());
                }
            }
        }
        std::fs::write(dir.join(format!("k{k}.raw")), &raw).unwrap();
        for (name, mode) in [("table", 2u8), ("off", 1)] {
            imij::lab::set_value_tables(mode);
            let file = imij::ImiEncoderConfig::default()
                .encode_samples(
                    imij::ImageRef::new(&raw, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            let (di, back) = imij::decode(&file).unwrap();
            assert!(di == info && back == raw, "k {k} {name} not exact");
            let maps = imij::lab::channel_maps(&file).unwrap();
            println!("k{k}\t{name}\t{}\t{maps:?}", file.len());
            std::fs::write(dir.join(format!("k{k}.{name}.imi")), &file).unwrap();
        }
    }
}
