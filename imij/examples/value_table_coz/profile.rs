//! Single-thread Coz progress points around exact decodes of retained files.
//!
//! Build with this directory's manifest and `--release --example value_table_coz`.
//! Run `coz run --- BINARY FILE.imi FILE.raw ITERATIONS` under the study's
//! resource and timing locks. Progress includes output clearing and the exact
//! pixel check, so use the paired `lowbit_eval` study to confirm speed changes.
//! The standalone manifest keeps the Coz dev dependency in this profiling tool.
//! Rust bindings: <https://github.com/plasma-umass/coz/tree/master/rust>.
#![forbid(unsafe_code)]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(args.len(), 4, "usage: value_table_coz FILE RAW ITERATIONS");
    let iterations: usize = args[3].parse().expect("positive iteration count");
    assert!(iterations > 0, "positive iteration count");
    coz::thread_init();
    let file = std::fs::read(&args[1]).expect("encoded input");
    let raw = std::fs::read(&args[2]).expect("reference pixels");
    let info = imij::probe(&file).expect("valid image header").0;
    let stride = info.row_bytes();
    assert_eq!(
        Some(raw.len()),
        stride.checked_mul(info.height as usize),
        "reference raster extent"
    );
    let mut out = vec![0; raw.len()];
    for _ in 0..iterations {
        out.fill(0);
        imij::decode_into(&file, &mut out, stride).expect("decode");
        assert_eq!(out, raw, "not exact");
        coz::progress!("checked-decode");
    }
    println!("{iterations} exact single-thread decodes");
}
