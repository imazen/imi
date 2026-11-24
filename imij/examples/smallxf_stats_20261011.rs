//! Small-transform study (2026-10-11): per lossy file, the 4x4 planes and
//! partition map bytes. `smallxf_stats_20261011 FILE...` prints
//! `file parents merged split0 split1 split2 map_bytes ac_bytes total_bytes`
//! and the flag-section bytes under the map grammar variants.

fn main() {
    for path in std::env::args().skip(1) {
        let data = std::fs::read(&path).expect("read");
        let s = imij::lab::lossy_small_stats(&data).expect("stats");
        let v = imij::lab::small_map_variants(&data).expect("variants");
        println!(
            "{path}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{v:?}",
            s[0],
            s[1],
            s[2],
            s[3],
            s[4],
            s[5],
            s[6],
            data.len()
        );
    }
}
